use hyper::{Response, StatusCode};

use crate::handlers::{full_body, BoxBody};

const SWAGGER_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Bifrost API Docs</title>
  <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css" />
  <style>
    html { box-sizing: border-box; overflow: -moz-scrollbars-vertical; overflow-y: scroll; }
    *, *:before, *:after { box-sizing: inherit; }
    body { margin: 0; }
    .swagger-ui .topbar { background-color: #1f2937; }
    .swagger-ui .topbar .download-url-wrapper .select-label { display: flex; align-items: center; }
    .swagger-ui .info .title { font-size: 28px; }
    .swagger-ui .scheme-container { box-shadow: none; }
  </style>
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js" crossorigin></script>
  <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-standalone-preset.js" crossorigin></script>
  <script>
    window.onload = function() {
      window.ui = SwaggerUIBundle({
        url: "./openapi.json",
        dom_id: '#swagger-ui',
        deepLinking: true,
        presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
        plugins: [SwaggerUIBundle.plugins.DownloadUrl],
        layout: "StandaloneLayout",
        defaultModelsExpandDepth: 1,
        defaultModelExpandDepth: 1,
        docExpansion: "list",
        filter: true,
        showExtensions: true,
        showCommonExtensions: true,
        tryItOutEnabled: true,
      });
    };
  </script>
</body>
</html>"#;

pub fn serve_swagger_ui() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(full_body(SWAGGER_HTML))
        .unwrap()
}

pub fn serve_openapi_spec() -> Response<BoxBody> {
    let spec = crate::openapi::generate_openapi_spec();
    let json = serde_json::to_string_pretty(&spec).unwrap_or_else(|_| "{}".to_string());

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(full_body(json))
        .unwrap()
}
