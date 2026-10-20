import type en from "../en/scripts";

const zhCN: Record<keyof typeof en, string> = {
  // Script types
  "type.request": "请求",
  "type.response": "响应",
  "type.decode": "解码",
  "type.parser": "解析",
  "typeShort.request": "请求",
  "typeShort.response": "响应",
  "typeShort.decode": "解码",
  "typeShort.parser": "解析",

  // Script list panel
  "list.title": "脚本",
  "list.newScript": "新建脚本",
  "list.moreActions": "更多操作",
  "list.searchPlaceholder": "搜索脚本...",
  "list.ariaLabel": "脚本列表",
  "list.noMatching": "没有匹配的脚本",
  "list.empty": "暂无脚本",
  "list.count": "{count} 个脚本",
  "list.createRequest": "请求脚本",
  "list.createResponse": "响应脚本",
  "list.createDecode": "解码脚本",
  "list.createParser": "解析脚本",
  "list.sandboxSettings": "沙箱设置",
  "list.exportAll": "全部导出",

  // Context menu
  "menu.newRequestScript": "新建请求脚本",
  "menu.newResponseScript": "新建响应脚本",
  "menu.newDecodeScript": "新建解码脚本",
  "menu.exportFolder": "导出文件夹（{count}）",
  "menu.deleteFolder": "删除文件夹（{count}）",
  "menu.exportCount": "导出（{count}）",
  "menu.deleteCount": "删除（{count}）",

  // Delete / rename
  "delete.scriptsTitle": "删除脚本",
  "delete.scriptsContent": "确定要删除这 {count} 个脚本吗？",
  "delete.deleteAll": "全部删除",
  "delete.scriptsDone": "已删除 {count} 个脚本",
  "delete.scriptTitle": "删除脚本",
  "delete.scriptContent": "确定要删除“{name}”吗？",
  "delete.scriptDone": "脚本已删除",
  "delete.noScriptsInFolder": "该文件夹中没有脚本",
  "delete.folderTitle": "删除文件夹",
  "delete.folderContent": "确定要删除文件夹“{folder}”及其中的 {count} 个脚本吗？",
  "delete.folderDone": "已删除 {count} 个脚本",
  "rename.title": "重命名脚本",
  "rename.success": "脚本已重命名",
  "rename.failed": "重命名脚本失败",
  "rename.placeholder": "输入新名称",

  // Editor empty state / help
  "editor.emptyDescription": "选择一个脚本或新建脚本",
  "editor.howToUse": "如何使用脚本",
  "editor.help.requestLabel": "请求脚本",
  "editor.help.requestText": "在转发到上游之前运行，可修改方法、请求头或请求体",
  "editor.help.responseLabel": "响应脚本",
  "editor.help.responseText": "在收到响应之后运行，可修改状态码、响应头或响应体",
  "editor.help.streamBefore": "响应脚本也可通过",
  "editor.help.streamAfter":
    "生成或转换真正增量的 SSE 流。流式脚本必须通过规则和实时 SSE 源进行测试，流式代码无法使用普通的“运行”操作。",
  "editor.help.logBefore": "使用",
  "editor.help.logAfter": "调试脚本",
  "editor.help.valuesBefore": "通过",
  "editor.help.valuesAfter": "读取自定义配置值",
  "editor.bindTitle": "将脚本绑定到规则",
  "editor.bindDescription":
    "对于 CLI、AI 生成或共享的规则，建议使用内联代码块，让规则自包含。本页面适合交互式测试和手动维护的具名脚本：",

  // Editor toolbar
  "editor.run": "运行",
  "editor.streamRunDisabled": "流式脚本需通过 resStreamScript 规则配合实时 SSE 源进行测试",

  // Test result panel
  "test.show": "显示测试结果",
  "test.hide": "隐藏测试结果",
  "test.result": "测试结果",
  "test.results": "测试结果",
  "test.resultWithDuration": "测试结果（{duration}ms）",
  "test.emptyHint": "运行测试后在此查看结果",
  "test.decodeOutput": "解码输出：",
  "test.requestModifications": "请求修改：",
  "test.responseModifications": "响应修改：",
  "test.logs": "日志：",
  "test.noLogs": "暂无日志",

  // Save / create
  "save.saved": "脚本已保存",
  "save.created": "脚本已创建",
  "save.nameRequired": "请输入脚本名称",
  "save.nameInvalid": "脚本名称只能包含字母、数字、连字符、下划线和斜杠",
  "save.newTitle": "保存新脚本",
  "save.namePlaceholder": "输入脚本名称（例如 api/add-auth-header）",
  "save.nameHint": "使用“/”创建文件夹。仅允许字母、数字、连字符、下划线和斜杠。",

  // Sandbox settings
  "sandbox.title": "沙箱设置",
  "sandbox.loadFailed": "加载沙箱配置失败",
  "sandbox.dirEmpty": "沙箱目录不能为空",
  "sandbox.positiveInt": "数值必须为正整数",
  "sandbox.allowedDirsAbsolute": "allowed_dirs 必须为绝对路径：{dir}",
  "sandbox.saved": "沙箱配置已保存",
  "sandbox.saveFailed": "保存沙箱配置失败（请检查目录是否为绝对路径）",
  "sandbox.netToggle": "网络请求（net.fetch）开关",
  "sandbox.dir": "沙箱目录（scripts 下的相对名称或绝对路径）",
  "sandbox.allowedDirs": "允许访问的系统目录（allowed_dirs，每行一个绝对路径）",
  "sandbox.fileMaxBytes": "最大文件大小（file.max_bytes）",
  "sandbox.netMaxRequestBytes": "最大请求体大小（net.max_request_bytes）",
  "sandbox.netMaxResponseBytes": "最大响应体大小（net.max_response_bytes）",
  "sandbox.netTimeout": "网络超时（net.timeout_ms）",
  "sandbox.allowPrivateNetwork": "允许访问私有网络（net.allow_private_network）",
  "sandbox.allowPrivateNetworkExtra":
    "默认关闭。仅当脚本必须访问回环地址、私有局域网、链路本地或元数据地址时才开启。",
  "sandbox.timeout": "脚本超时（limits.timeout_ms）",
  "sandbox.maxMemory": "脚本最大内存（limits.max_memory_bytes）",
  "sandbox.maxDecodeInput": "最大解码输入大小（limits.max_decode_input_bytes）",
  "sandbox.maxDecompressOutput": "HTTP 解压输出上限（limits.max_decompress_output_bytes）",
};

export default zhCN;
