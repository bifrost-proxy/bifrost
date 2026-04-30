use super::{now_ms, NetworkEvent, NetworkEventInput};

pub(super) fn network_event_from_input(event: NetworkEventInput) -> Option<NetworkEvent> {
    if event.url.trim().is_empty() {
        return None;
    }
    let client_req_id = event
        .client_req_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Some(NetworkEvent {
        url: event.url,
        method: event.method.unwrap_or_else(|| "GET".to_string()),
        status: event.status,
        resource_type: event.resource_type.unwrap_or_else(|| "Other".to_string()),
        at_ms: now_ms(),
        query_params: event.query_params,
        request_headers: event.request_headers,
        response_headers: event.response_headers,
        from_cache: event.from_cache,
        client_req_id,
        traffic_id: None,
    })
}

pub(super) fn merge_network_events(target: &mut Vec<NetworkEvent>, events: Vec<NetworkEvent>) {
    for event in events {
        merge_network_event(target, event);
    }
}

pub(super) fn merge_network_event(target: &mut Vec<NetworkEvent>, event: NetworkEvent) {
    if let Some(client_req_id) = event.client_req_id.as_deref() {
        if let Some(existing) = target
            .iter_mut()
            .rev()
            .find(|item| item.client_req_id.as_deref() == Some(client_req_id))
        {
            merge_network_event_fields(existing, event);
            return;
        }
    }

    let fallback_key = network_event_fallback_key(&event);
    if event.client_req_id.is_none()
        && target.iter().rev().take(80).any(|item| {
            item.client_req_id.is_some() && network_event_fallback_key(item) == fallback_key
        })
    {
        return;
    }

    if event.client_req_id.is_some() {
        target.retain(|item| {
            item.client_req_id.is_some() || network_event_fallback_key(item) != fallback_key
        });
    } else if let Some(existing) = target.iter_mut().rev().find(|item| {
        item.client_req_id.is_none() && network_event_fallback_key(item) == fallback_key
    }) {
        merge_network_event_fields(existing, event);
        return;
    }

    target.push(event);
}

fn merge_network_event_fields(existing: &mut NetworkEvent, event: NetworkEvent) {
    if event.status.is_some() {
        existing.status = event.status;
    }
    if !event.resource_type.trim().is_empty() && event.resource_type != "Other" {
        existing.resource_type = event.resource_type;
    }
    if !event.query_params.is_empty() {
        existing.query_params = event.query_params;
    }
    if !event.request_headers.is_empty() {
        existing.request_headers = event.request_headers;
    }
    if !event.response_headers.is_empty() {
        existing.response_headers = event.response_headers;
    }
    if event.from_cache.is_some() {
        existing.from_cache = event.from_cache;
    }
    if event.traffic_id.is_some() {
        existing.traffic_id = event.traffic_id;
    }
    if event.client_req_id.is_some() {
        existing.client_req_id = event.client_req_id;
    }
    existing.method = event.method;
    existing.url = strip_internal_devtools_client_req_id(&event.url);
    existing.at_ms = event.at_ms;
}

fn network_event_fallback_key(event: &NetworkEvent) -> String {
    format!(
        "{} {}",
        event.method.to_ascii_uppercase(),
        strip_internal_devtools_client_req_id(&event.url)
    )
}

fn strip_internal_devtools_client_req_id(raw_url: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(raw_url) {
        parsed.set_fragment(None);
        if parsed
            .query_pairs()
            .any(|(key, _)| key == "__bifrost_client_req_id")
        {
            let pairs: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(key, _)| key != "__bifrost_client_req_id")
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            parsed.set_query(None);
            if !pairs.is_empty() {
                parsed
                    .query_pairs_mut()
                    .extend_pairs(pairs.iter().map(|(key, value)| (&**key, &**value)));
            }
        }
        return parsed.to_string();
    }

    let without_fragment = raw_url.split('#').next().unwrap_or(raw_url);
    let Some((path, query)) = without_fragment.split_once('?') else {
        return without_fragment.to_string();
    };
    let filtered: Vec<&str> = query
        .split('&')
        .filter(|part| {
            part.split_once('=')
                .map(|(key, _)| key != "__bifrost_client_req_id")
                .unwrap_or(*part != "__bifrost_client_req_id")
        })
        .collect();
    if filtered.is_empty() {
        path.to_string()
    } else {
        format!("{}?{}", path, filtered.join("&"))
    }
}
