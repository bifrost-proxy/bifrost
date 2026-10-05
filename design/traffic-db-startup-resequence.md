# Traffic DB 启动序列号与持久化身份

## 目标与兼容性

Traffic 列表、分页和 Push 使用 SQLite 的 `traffic_records.sequence` 排序。
同一数据库重启不重排记录；清空或删除尾部后重启也不能复用已持久化的序号。
仅靠数值回退不能识别替换的数据库，因为新库可能有相同或更大的计数。

现有 `metadata(key TEXT, value TEXT)` 增加两个键，不新增表、不提升
`SCHEMA_VERSION`、不丢弃旧记录，也不改变主键或已有字段：

- `database_epoch`：首次打开时生成的 UUID，之后作为不透明字符串保存。
- `sequence_high_water`：删除前保存的已分配序号上界；没有删除时可以不存在。

兼容旧库第一次打开时只补身份；已有序号、详情与统计继续保留。
身份初始化失败直接返回错误，不能进入现有 schema 恢复的清库分支。

## 生命周期

- 新数据库取得新身份；同一个路径重建数据库也取得新身份。
- 正常重启、clear、按 ID 删除、保留活跃连接的 clear、过期清理、数量或容量
  清理和 VACUUM 保持同一身份。
- 数据库副本保留其 metadata 身份；此字段表示数据库序列历史，不是进程或路径。
- 失败的插入可能只消耗内存序号，因此重启后的 `server_sequence` 可以变小。
  相同的已知身份优先于数值回退判断；这不会复用曾经成功持久化的记录序号。
- 老版本服务没有身份字段，客户端继续使用原有兼容恢复逻辑。

## 实现

`TrafficDbStore::new()` 先完成既有 schema 初始化，再运行独立 metadata 事务。
`INSERT OR IGNORE` 后读取实际保存的身份，避免并发初始化生成不同身份。
空身份、无效 high-water 或无法分配下一个 SQLite 整数序号时返回错误并保留文件。

启动时 `current_sequence = max(MAX(sequence), sequence_high_water) + 1`。
全新空库从 1 开始；已清空的库从已保存 high-water 之后继续。
记录写入仍由 `AtomicU64::fetch_add` 分配，不给每个请求增加 metadata 写入。

所有删除入口在 writer 锁内先提交 high-water，再删除记录。即使进程在两步之间
退出，结果最多保留空洞，不会把已删除的序号重新分配。保存失败时不删除；
分批清理遇到零进展就停止，避免反复查询同一批记录。

HTTP `/traffic/statistics`、`/traffic/updates` 和 Push `traffic_delta` 增加可选的
`database_epoch` 字符串。真实 store 总是返回身份；缺少 store 或旧服务可以省略。
Rust 反序列化接受没有新字段的旧消息。Updates/delta 同批携带身份，避免旧库首屏
记录与新库随后到达的统计绑定成同一个缓存。客户端仍需新鲜请求确认身份变化，
拒绝失效请求，并重建有界窗口、分页边界和筛选扫描。

## 验证

`traffic_db::store` 回归测试覆盖：

- 同库重启、新库及同路径重建，旧记录保留和旧 metadata 回填。
- 并发初始化取得同一个持久身份。
- clear、保留活跃连接、尾部删除、过期删除和数量清理后重启不复用序号。
- 初始化失败或损坏 metadata 不清库；删除前写入失败保留记录并停止清理。
- 插入失败后重启允许未持久化计数回退，身份和已有记录保持不变。

统计和 delta 测试验证可选字段序列化与旧消息反序列化。
实际验证结果和资源受限未运行项目记录在交付证据中；CI 仍须执行 admin 测试、
端到端验证和 coverage 门禁，局部 harness 不替代完整门禁。
