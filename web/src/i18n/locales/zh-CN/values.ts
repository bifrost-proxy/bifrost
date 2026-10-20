import type en from "../en/values";

const zhCN: Record<keyof typeof en, string> = {
  // ValueList
  "list.title": "变量值",
  "list.newValue": "新建变量值",
  "list.exportAll": "全部导出",
  "list.searchPlaceholder": "搜索变量值…",
  "list.ariaLabel": "变量值列表",
  "list.emptyNoMatch": "没有匹配的变量值",
  "list.emptyNoValues": "暂无变量值",
  "list.stats": "共 {count} 个变量值",
  "list.sortNewest": "最新创建",
  "list.sortUpdated": "最近更新",
  "list.sortName": "名称",
  "list.unsavedChanges": "有未保存的更改",
  "list.menuCopyValue": "复制值",
  "list.menuExportCount": "导出（{count}）",
  "list.menuDeleteCount": "删除（{count}）",
  "list.nameRequired": "请输入变量值名称",
  "list.namePlaceholder": "变量值名称（如 api_key、auth_token）",
  "list.newNamePlaceholder": "新名称",
  "list.created": "变量值已创建",
  "list.renameTitle": "重命名变量值",
  "list.renamed": "变量值已重命名",
  "list.copiedToClipboard": "已复制“{name}”到剪贴板",
  "list.bulkDeleteTitle": "删除变量值",
  "list.bulkDeleteContent": "确定删除 {count} 个变量值吗？",
  "list.bulkDeletedOne": "已删除 {count} 个变量值",
  "list.bulkDeletedOther": "已删除 {count} 个变量值",

  // Shared (ValueList + ValueEditor)
  "deleteTitle": "删除变量值",
  "deleteContent": "确定删除“{name}”吗？",
  "deleted": "变量值已删除",
  "copyFailed": "复制失败",

  // ValueEditor
  "editor.format": "格式化",
  "editor.formatted": "已格式化",
  "editor.selectToEdit": "选择一个变量值进行编辑",
  "editor.guideWhatTitle": "什么是变量值？",
  "editor.guideWhatBody":
    "变量值是可在规则中引用的可复用变量，可用于存放 API Key、Token 等敏感数据或常用内容。",
  "editor.guideHowTitle": "使用方法",
  "editor.guideHowCreatePrefix": "创建一个名称唯一的变量值（如 ",
  "editor.guideHowCreateSuffix": "）",
  "editor.guideHowReferencePrefix": "在规则中使用 ",
  "editor.guideHowReferenceSuffix": " 语法引用",
  "editor.guideHowExamplePrefix": "示例：",
  "editor.guideHowExampleSuffix": " 会被替换为实际的值",
  "editor.guideTipsTitle": "提示",
  "editor.guideTipNames": "使用有描述性的名称，便于识别",
  "editor.guideTipHighlight": "自动识别 JSON 和 XML 内容并进行语法高亮",
  "editor.guideTipSavePrefix": "按 ",
  "editor.guideTipSaveSuffix": " 快速保存更改",
  "editor.hintPrefix": "在规则中使用 ",
  "editor.hintSuffix": " 引用此变量值",
};

export default zhCN;
