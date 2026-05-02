#[cfg(feature = "http3")]
use bifrost_core::text::truncate_chars_with_suffix;
#[cfg(feature = "http3")]
use bifrost_proxy::http3::Http3Client;
use bytes::Bytes;
use hyper::Request;

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    std::env::set_var("RUST_LOG", "bifrost_proxy=info,h3=debug,quinn=info");
    env_logger::init();

    println!("=== HTTP/3 Client Test ===\n");

    #[cfg(feature = "http3")]
    {
        let client = match Http3Client::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to create HTTP/3 client: {}", e);
                return;
            }
        };

        println!("Testing HTTP/3 connection to edith.xiaohongshu.com...\n");

        let req = Request::builder()
            .method("GET")
            .uri("https://edith.xiaohongshu.com/api/sns/web/global/config")
            .header("Host", "edith.xiaohongshu.com")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
            )
            .header("Accept", "application/json")
            .body(Bytes::new())
            .unwrap();

        match client.request("edith.xiaohongshu.com", 443, req).await {
            Ok(response) => {
                println!("\n=== Response ===");
                println!("Status: {}", response.status());
                println!("Headers:");
                for (key, value) in response.headers() {
                    println!("  {}: {:?}", key, value);
                }
                println!("\nBody length: {} bytes", response.body().len());

                if let Ok(body_str) = std::str::from_utf8(response.body()) {
                    let preview = truncate_chars_with_suffix(body_str, 500, "");
                    println!("Body preview:\n{}", preview);
                }

                println!("\n✅ HTTP/3 connection successful!");
            }
            Err(e) => {
                eprintln!("\n❌ HTTP/3 request failed: {}", e);
            }
        }
    }

    #[cfg(not(feature = "http3"))]
    {
        eprintln!("HTTP/3 feature is not enabled. Run with --features http3");
    }
}
