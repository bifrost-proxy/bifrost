use super::config::ResearchFetchPolicy;
use super::normalize::content_hash;
use super::provider::{FetchedDocument, FetchedDocumentInput, ResearchFetchRequest};
use anyhow::{anyhow, Context};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use url::Url;

pub async fn fetch_document(
    client: &reqwest::Client,
    policy: &ResearchFetchPolicy,
    req: ResearchFetchRequest,
) -> anyhow::Result<FetchedDocument> {
    let max_bytes = req
        .max_bytes
        .unwrap_or(policy.max_response_bytes)
        .min(policy.max_response_bytes);
    validate_url_allowed(&req.url, policy).await?;
    let mut current = req.url.clone();
    let mut redirects = 0usize;

    loop {
        validate_url_allowed(&current, policy).await?;
        let response = client
            .get(&current)
            .timeout(Duration::from_secs(policy.timeout_secs))
            .header(reqwest::header::USER_AGENT, policy.user_agent.clone())
            .send()
            .await
            .with_context(|| format!("fetch {}", current))?;

        if response.status().is_redirection() {
            redirects += 1;
            if redirects > policy.max_redirects {
                return Err(anyhow!("too many redirects"));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("redirect without location"))?;
            current = Url::parse(&current)
                .and_then(|base| base.join(location))
                .map(|url| url.to_string())
                .map_err(|error| anyhow!("invalid redirect: {error}"))?;
            continue;
        }

        if !response.status().is_success() {
            return Err(anyhow!("fetch failed with status {}", response.status()));
        }

        if let Some(len) = response.content_length() {
            if len as usize > max_bytes {
                return Err(anyhow!("response too large: {} bytes", len));
            }
        }

        let final_url = response.url().to_string();
        validate_url_allowed(&final_url, policy).await?;
        let bytes = read_limited(response, max_bytes).await?;
        let text = String::from_utf8_lossy(&bytes).to_string();
        let title = extract_title(&text);
        let markdown = html_to_markdownish(&text);
        let hash = content_hash(&markdown);
        let now = chrono_like_now();
        return Ok(FetchedDocument::from_markdown(FetchedDocumentInput {
            url: final_url,
            source: None,
            provider: None,
            site_name: None,
            title,
            author: None,
            published_at: None,
            content_markdown: markdown,
            content_hash: hash,
            retrieved_at: now,
        }));
    }
}

async fn read_limited(response: reqwest::Response, max_bytes: usize) -> anyhow::Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if out.len() + chunk.len() > max_bytes {
            return Err(anyhow!("response exceeded {} bytes", max_bytes));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

pub async fn validate_url_allowed(
    raw_url: &str,
    policy: &ResearchFetchPolicy,
) -> anyhow::Result<()> {
    let url = Url::parse(raw_url).map_err(|error| anyhow!("invalid url: {error}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(anyhow!("unsupported url scheme: {other}")),
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("url has no host"))?
        .to_string();
    if !policy.allow_localhost && is_localhost_name(&host) {
        return Err(anyhow!("localhost fetch is disabled"));
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = tokio::net::lookup_host((host.as_str(), port))
        .await
        .with_context(|| format!("resolve {}", host))?;
    for addr in addrs {
        validate_ip_allowed(addr.ip(), policy)?;
    }
    Ok(())
}

fn validate_ip_allowed(ip: IpAddr, policy: &ResearchFetchPolicy) -> anyhow::Result<()> {
    if !policy.allow_localhost && ip.is_loopback() {
        return Err(anyhow!("loopback fetch is disabled"));
    }
    if !policy.allow_private_ip && is_private_like(ip) {
        return Err(anyhow!("private network fetch is disabled"));
    }
    Ok(())
}

fn is_localhost_name(host: &str) -> bool {
    matches!(
        host.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "localhost" | "localhost.localdomain"
    )
}

fn is_private_like(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4 == Ipv4Addr::UNSPECIFIED
        }
        IpAddr::V6(v6) => {
            v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_unspecified()
                || is_ipv6_documentation(v6)
        }
    }
}

fn is_ipv6_documentation(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] == 0x2001) && (ip.segments()[1] == 0x0db8)
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let start_close = lower[start..].find('>')? + start + 1;
    let end = lower[start_close..].find("</title>")? + start_close;
    let title = html[start_close..end].trim();
    (!title.is_empty()).then(|| decode_basic_entities(title))
}

fn html_to_markdownish(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut last_space = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                if !last_space {
                    out.push('\n');
                    last_space = true;
                }
            }
            '>' => in_tag = false,
            _ if in_tag => {}
            ch if ch.is_whitespace() => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            ch => {
                out.push(ch);
                last_space = false;
            }
        }
    }
    decode_basic_entities(&out)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn decode_basic_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn chrono_like_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_localhost_by_default() {
        let policy = ResearchFetchPolicy::default();
        let result = validate_url_allowed("http://localhost:8080/a", &policy).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_private_ip_by_default() {
        let policy = ResearchFetchPolicy::default();
        let result = validate_url_allowed("http://127.0.0.1:8080/a", &policy).await;
        assert!(result.is_err());
    }
}
