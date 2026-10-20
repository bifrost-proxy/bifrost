const en = {
  // Script types
  "type.request": "Request",
  "type.response": "Response",
  "type.decode": "Decode",
  "type.parser": "Parser",
  "typeShort.request": "REQ",
  "typeShort.response": "RES",
  "typeShort.decode": "DEC",
  "typeShort.parser": "PAR",

  // Script list panel
  "list.title": "Scripts",
  "list.newScript": "New Script",
  "list.moreActions": "More Actions",
  "list.searchPlaceholder": "Search scripts...",
  "list.ariaLabel": "Scripts list",
  "list.noMatching": "No matching scripts",
  "list.empty": "No scripts yet",
  "list.count": "{count} scripts",
  "list.createRequest": "Request Script",
  "list.createResponse": "Response Script",
  "list.createDecode": "Decode Script",
  "list.createParser": "Parser Script",
  "list.sandboxSettings": "Sandbox Settings",
  "list.exportAll": "Export All",

  // Context menu
  "menu.newRequestScript": "New Request Script",
  "menu.newResponseScript": "New Response Script",
  "menu.newDecodeScript": "New Decode Script",
  "menu.exportFolder": "Export Folder ({count})",
  "menu.deleteFolder": "Delete Folder ({count})",
  "menu.exportCount": "Export ({count})",
  "menu.deleteCount": "Delete ({count})",

  // Delete / rename
  "delete.scriptsTitle": "Delete Scripts",
  "delete.scriptsContent": "Are you sure you want to delete {count} scripts?",
  "delete.deleteAll": "Delete All",
  "delete.scriptsDone": "Deleted {count} scripts",
  "delete.scriptTitle": "Delete Script",
  "delete.scriptContent": "Are you sure you want to delete \"{name}\"?",
  "delete.scriptDone": "Script deleted",
  "delete.noScriptsInFolder": "No scripts in this folder",
  "delete.folderTitle": "Delete Folder",
  "delete.folderContent":
    "Are you sure you want to delete folder \"{folder}\" and all {count} script(s) inside?",
  "delete.folderDone": "Deleted {count} script(s)",
  "rename.title": "Rename Script",
  "rename.success": "Script renamed",
  "rename.failed": "Failed to rename script",
  "rename.placeholder": "Enter new name",

  // Editor empty state / help
  "editor.emptyDescription": "Select a script or create a new one",
  "editor.howToUse": "How to use Scripts",
  "editor.help.requestLabel": "Request scripts",
  "editor.help.requestText": "run before forwarding to upstream - modify method, headers, or body",
  "editor.help.responseLabel": "Response scripts",
  "editor.help.responseText": "run after receiving response - modify status, headers, or body",
  "editor.help.streamBefore": "Response scripts can also be used by",
  "editor.help.streamAfter":
    "to generate or transform a true incremental SSE stream. Stream scripts must be tested through a rule and live SSE source; the ordinary Run action is disabled for stream code.",
  "editor.help.logBefore": "Use",
  "editor.help.logAfter": "to debug your scripts",
  "editor.help.valuesBefore": "Access",
  "editor.help.valuesAfter": "for custom configuration values",
  "editor.bindTitle": "Bind scripts to rules",
  "editor.bindDescription":
    "For CLI, AI-generated, or shared rules, prefer an inline block so the rule is self-contained. This page is useful for interactive testing and manually maintained named scripts:",

  // Editor toolbar
  "editor.run": "Run",
  "editor.streamRunDisabled":
    "Stream scripts are tested through a resStreamScript rule with a live SSE source",

  // Test result panel
  "test.show": "Show Test Results",
  "test.hide": "Hide Test Results",
  "test.result": "Test Result",
  "test.results": "Test Results",
  "test.resultWithDuration": "Test Result ({duration}ms)",
  "test.emptyHint": "Run a test to see results here",
  "test.decodeOutput": "Decode Output:",
  "test.requestModifications": "Request Modifications:",
  "test.responseModifications": "Response Modifications:",
  "test.logs": "Logs:",
  "test.noLogs": "No logs",

  // Save / create
  "save.saved": "Script saved",
  "save.created": "Script created",
  "save.nameRequired": "Please enter a script name",
  "save.nameInvalid":
    "Script name can only contain letters, numbers, hyphens, underscores and slashes",
  "save.newTitle": "Save New Script",
  "save.namePlaceholder": "Enter script name (e.g., api/add-auth-header)",
  "save.nameHint":
    "Use \"/\" to create folders. Only letters, numbers, hyphens, underscores and slashes are allowed.",

  // Sandbox settings
  "sandbox.title": "Sandbox Settings",
  "sandbox.loadFailed": "Failed to load Sandbox config",
  "sandbox.dirEmpty": "Sandbox directory cannot be empty",
  "sandbox.positiveInt": "Numeric values must be positive integers",
  "sandbox.allowedDirsAbsolute": "allowed_dirs must be absolute paths: {dir}",
  "sandbox.saved": "Sandbox config saved",
  "sandbox.saveFailed":
    "Failed to save Sandbox config (check if directories are absolute paths)",
  "sandbox.netToggle": "Network Request (net.fetch) Toggle",
  "sandbox.dir": "Sandbox Directory (relative name under scripts, or absolute path)",
  "sandbox.allowedDirs": "Allowed System Directories (allowed_dirs, one absolute path per line)",
  "sandbox.fileMaxBytes": "Max File Size (file.max_bytes)",
  "sandbox.netMaxRequestBytes": "Max Request Body Size (net.max_request_bytes)",
  "sandbox.netMaxResponseBytes": "Max Response Body Size (net.max_response_bytes)",
  "sandbox.netTimeout": "Network Timeout (net.timeout_ms)",
  "sandbox.allowPrivateNetwork": "Allow Private Network Fetch (net.allow_private_network)",
  "sandbox.allowPrivateNetworkExtra":
    "Off by default. Enable only for scripts that must call loopback, private LAN, link-local, or metadata addresses.",
  "sandbox.timeout": "Script Timeout (limits.timeout_ms)",
  "sandbox.maxMemory": "Script Max Memory (limits.max_memory_bytes)",
  "sandbox.maxDecodeInput": "Max Decode Input Size (limits.max_decode_input_bytes)",
  "sandbox.maxDecompressOutput":
    "Max HTTP Decompress Output Size (limits.max_decompress_output_bytes)",
};

export default en;
