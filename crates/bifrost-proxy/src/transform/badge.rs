use bytes::Bytes;

pub const BIFROST_BADGE_ELEMENT_ID: &str = "__bifrost_badge__";

const BADGE_STYLE: &str = concat!(
    "<style>",
    "#__bifrost_badge__{",
    "position:fixed;left:15px;bottom:15px;z-index:2147483647!important;",
    "display:flex;align-items:center;",
    "height:30px;width:30px;border-radius:9999px;",
    "background:linear-gradient(135deg,#7BEBC0,#6CBFCF);",
    "box-shadow:0 0 10px 2px rgba(123,235,192,0.35),0 0 0 2px rgba(255,255,255,0.9);",
    "cursor:pointer;overflow:hidden;white-space:nowrap;",
    "font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;",
    "user-select:none;-webkit-user-select:none;",
    "transition:width .4s cubic-bezier(.4,0,.2,1),border-radius .4s cubic-bezier(.4,0,.2,1),box-shadow .3s;",
    "}",
    "#__bifrost_badge__:hover{",
    "width:220px;border-radius:15px;",
    "box-shadow:0 0 18px 4px rgba(123,235,192,0.45),0 0 0 2px rgba(255,255,255,0.95);",
    "}",
    "#__bifrost_badge__ .__bb_ico{",
    "min-width:30px;width:30px;height:30px;",
    "display:flex;align-items:center;justify-content:center;",
    "font-size:14px;font-weight:800;color:#fff;line-height:1;",
    "}",
    "#__bifrost_badge__ .__bb_txt{",
    "font-size:12px;font-weight:600;color:#fff;",
    "opacity:0;padding-right:14px;",
    "transition:opacity .25s .1s;",
    "}",
    "#__bifrost_badge__:hover .__bb_txt{opacity:1}",
    "#__bb_panel__{",
    "position:fixed;left:15px;bottom:52px;z-index:2147483647!important;",
    "min-width:280px;max-width:400px;max-height:420px;",
    "background:#fff;border-radius:12px;",
    "box-shadow:0 8px 32px rgba(0,0,0,0.12),0 2px 8px rgba(0,0,0,0.08);",
    "font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;",
    "overflow:hidden auto;opacity:0;transform:translateY(8px) scale(0.96);",
    "pointer-events:none;transition:opacity .2s ease,transform .2s ease;",
    "}",
    "#__bb_panel__.--visible{",
    "opacity:1;transform:translateY(0) scale(1);pointer-events:auto;",
    "}",
    "#__bb_panel__ .__bb_ph{",
    "padding:12px 16px 8px;font-size:11px;font-weight:700;",
    "color:#6CBFCF;letter-spacing:.5px;text-transform:uppercase;",
    "border-bottom:1px solid #f0f0f0;display:flex;align-items:center;gap:6px;",
    "position:sticky;top:0;background:inherit;z-index:1;",
    "}",
    "#__bb_panel__ .__bb_ph svg{width:14px;height:14px;fill:#6CBFCF}",
    "#__bb_panel__ .__bb_pl{",
    "padding:4px 0;",
    "}",
    "#__bb_panel__ .__bb_sec{",
    "padding:8px 16px 4px;font-size:10px;font-weight:600;",
    "color:#999;text-transform:uppercase;letter-spacing:.3px;",
    "}",
    "#__bb_panel__ .__bb_ri{",
    "padding:8px 16px;font-size:13px;color:#333;",
    "display:flex;align-items:center;gap:8px;",
    "transition:background .15s;cursor:pointer;text-decoration:none;",
    "}",
    "#__bb_panel__ .__bb_ri:hover{background:#f7f7f7}",
    "#__bb_panel__ .__bb_ri .__bb_dot{",
    "width:6px;height:6px;border-radius:50%;background:#52c41a;flex-shrink:0;",
    "}",
    "#__bb_panel__ .__bb_ri .__bb_rn{",
    "flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
    "}",
    "#__bb_panel__ .__bb_ri .__bb_rc{",
    "font-size:11px;color:#999;flex-shrink:0;",
    "}",
    "#__bb_panel__ .__bb_empty{",
    "padding:24px 16px;text-align:center;font-size:13px;color:#999;",
    "}",
    "#__bb_panel__ .__bb_grp{",
    "padding:8px 16px 4px;font-size:10px;font-weight:600;color:#999;",
    "display:flex;align-items:center;gap:4px;",
    "}",
    "#__bb_panel__ .__bb_grp svg{width:11px;height:11px;fill:#999}",
    "#__bb_panel__ .__bb_mg{",
    "padding:8px 16px;border-top:1px solid #f0f0f0;",
    "}",
    "#__bb_panel__ .__bb_mg .__bb_mt{",
    "font-size:11px;font-weight:600;color:#6CBFCF;cursor:pointer;",
    "display:flex;align-items:center;gap:4px;user-select:none;",
    "}",
    "#__bb_panel__ .__bb_mg .__bb_mt:hover{opacity:.8}",
    "#__bb_panel__ .__bb_mg .__bb_mc{",
    "padding:28px 8px 8px;background:#f5f5f5;border-radius:6px;",
    "font-size:11px;font-family:'SF Mono',Menlo,Consolas,monospace;",
    "color:#333;white-space:pre-wrap;word-break:break-all;",
    "}",
    "#__bb_panel__ .__bb_mg .__bb_mcw{",
    "display:none;position:relative;margin-top:8px;",
    "}",
    "#__bb_panel__ .__bb_mg .__bb_mcw.--open{display:block}",
    "#__bb_panel__ .__bb_mg .__bb_copy{",
    "position:absolute;right:6px;top:6px;z-index:2;",
    "border:1px solid rgba(108,191,207,0.45);border-radius:4px;",
    "background:rgba(255,255,255,0.92);color:#2f9aae;",
    "font-size:10px;font-weight:600;line-height:18px;height:20px;padding:0 7px;",
    "font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;",
    "cursor:pointer;",
    "}",
    "#__bb_panel__ .__bb_mg .__bb_copy:hover{background:#fff;border-color:#6CBFCF}",
    "@media(prefers-color-scheme:dark){",
    "#__bb_panel__{background:#1f1f1f;box-shadow:0 8px 32px rgba(0,0,0,0.4),0 2px 8px rgba(0,0,0,0.3)}",
    "#__bb_panel__ .__bb_ph{color:#7BEBC0;border-bottom-color:#333}",
    "#__bb_panel__ .__bb_ph svg{fill:#7BEBC0}",
    "#__bb_panel__ .__bb_ri{color:#e0e0e0}",
    "#__bb_panel__ .__bb_ri:hover{background:#2a2a2a}",
    "#__bb_panel__ .__bb_sec{color:#666}",
    "#__bb_panel__ .__bb_ri .__bb_rc{color:#666}",
    "#__bb_panel__ .__bb_empty{color:#666}",
    "#__bb_panel__ .__bb_grp{color:#666}",
    "#__bb_panel__ .__bb_grp svg{fill:#666}",
    "#__bb_panel__ .__bb_mg{border-top-color:#333}",
    "#__bb_panel__ .__bb_mg .__bb_mc{background:#2a2a2a;color:#ccc}",
    "#__bb_panel__ .__bb_mg .__bb_copy{background:rgba(31,31,31,0.92);color:#7BEBC0;border-color:rgba(123,235,192,0.35)}",
    "#__bb_panel__ .__bb_mg .__bb_copy:hover{background:#262626;border-color:#7BEBC0}",
    "}",
    "</style>",
);

const BADGE_HTML: &str = concat!(
    r#"<div id="__bifrost_badge__" aria-hidden="true">"#,
    r#"<span class="__bb_ico">B</span>"#,
    r#"<span class="__bb_txt">Bifrost proxy is working</span>"#,
    "</div>",
    r#"<div id="__bb_panel__"></div>"#,
);

fn badge_script(rules_json: &str) -> String {
    format!(
        concat!(
            "<script>",
            "(function(){{",
            "var B=document.getElementById('__bifrost_badge__');",
            "var P=document.getElementById('__bb_panel__');",
            "if(!B||!P)return;",
            "var D={rules_json};",
            "var hideTimer=null;",
            "var base=D.admin_port?'http://127.0.0.1:'+D.admin_port+'/_bifrost/rules':'';",
            "var BOLT='<svg viewBox=\"0 0 1024 1024\" xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M560 192L256 576h208l-48 256 320-384H528l32-256z\" fill=\"currentColor\"/></svg>';",
            "var TEAM='<svg viewBox=\"0 0 1024 1024\" xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M824 512a56 56 0 1 0 0-112 56 56 0 0 0 0 112zm-312-88a120 120 0 1 0 0-240 120 120 0 0 0 0 240zm-312 88a56 56 0 1 0 0-112 56 56 0 0 0 0 112zm624 56c-46 0-86 26-106 64h-4c-24-48-62-86-108-110a184 184 0 0 0-104-32h-8a184 184 0 0 0-104 32c-46 24-84 62-108 110h-4a120 120 0 0 0-106-64c-66 0-120 54-120 120v64h248a248 248 0 0 0 8 24h-8v200h416V792h-8a248 248 0 0 0 8-24h248v-64c0-66-54-120-120-120z\" fill=\"currentColor\"/></svg>';",
            "function show(){{clearTimeout(hideTimer);render();P.classList.add('--visible')}}",
            "function hide(){{hideTimer=setTimeout(function(){{P.classList.remove('--visible')}},150)}}",
            "function esc(s){{var d=document.createElement('div');d.textContent=s;return d.innerHTML}}",
            "function setCopyState(btn,text){{btn.textContent=text;btn.setAttribute('aria-label',text==='Copy'?'Copy merged rules':text)}}",
            "function fallbackCopy(text){{return new Promise(function(resolve,reject){{var t=document.createElement('textarea');var prev=document.activeElement;var ok=false;var copied=false;function onCopy(e){{if(e.clipboardData){{e.clipboardData.setData('text/plain',text);e.preventDefault();copied=true}}}}t.value=text;t.setAttribute('readonly','');t.style.position='fixed';t.style.top='0';t.style.left='0';t.style.width='1px';t.style.height='1px';t.style.opacity='0';document.body.appendChild(t);document.addEventListener('copy',onCopy,true);try{{t.focus();t.select();t.setSelectionRange(0,text.length);ok=document.execCommand('copy')}}catch(e){{ok=false}}document.removeEventListener('copy',onCopy,true);document.body.removeChild(t);try{{if(prev&&prev.focus)prev.focus()}}catch(e){{}}ok&&copied?resolve():reject(new Error('copy failed'))}})}}",
            "function copyText(text){{if(navigator.clipboard&&window.isSecureContext&&navigator.clipboard.writeText){{return navigator.clipboard.writeText(text).catch(function(){{return fallbackCopy(text)}})}}return fallbackCopy(text)}}",
            "function copyMerged(btn){{copyText(String(D.merged_content||'').trim()).then(function(){{setCopyState(btn,'Copied');setTimeout(function(){{setCopyState(btn,'Copy')}},1200)}}).catch(function(){{setCopyState(btn,'Failed');setTimeout(function(){{setCopyState(btn,'Copy')}},1600)}})}}",
            "function ruleRow(r){{",
            "var href='';",
            "if(base){{",
            "if(r.group_id){{href=base+'?group='+encodeURIComponent(r.group_name||r.group_id)+'&rule='+encodeURIComponent(r.name)}}",
            "else{{href=base+'?rule='+encodeURIComponent(r.name)}}",
            "}}",
            "var tag=href?'a':'div';",
            "var extra=href?' href=\"'+href+'\" target=\"_blank\" rel=\"noopener\"':'';",
            "return'<'+tag+' class=\"__bb_ri\"'+extra+'><span class=\"__bb_dot\"></span><span class=\"__bb_rn\">'+esc(r.name)+'</span><span class=\"__bb_rc\">'+r.rule_count+' rules</span></'+tag+'>';",
            "}}",
            "function render(){{",
            "var rules=D.rules||[];",
            "var html='<div class=\"__bb_ph\">'+BOLT+' Active Rules<span style=\"margin-left:auto;font-size:12px;font-weight:500;color:#52c41a\">'+rules.length+' active</span></div>';",
            "html+='<div class=\"__bb_pl\">';",
            "if(rules.length===0){{html+='<div class=\"__bb_empty\">No active rules</div>'}}",
            "else{{",
            "var own=rules.filter(function(r){{return!r.group_id}});",
            "var groups={{}};",
            "rules.forEach(function(r){{if(r.group_id){{if(!groups[r.group_id])groups[r.group_id]={{name:r.group_id,rules:[]}};groups[r.group_id].rules.push(r)}}}});",
            "if(own.length>0){{",
            "html+='<div class=\"__bb_sec\">My Rules</div>';",
            "own.forEach(function(r){{html+=ruleRow(r)}});",
            "}}",
            "Object.keys(groups).forEach(function(gid){{",
            "var g=groups[gid];",
            "html+='<div class=\"__bb_grp\">'+TEAM+' '+esc(g.name)+'</div>';",
            "g.rules.forEach(function(r){{html+=ruleRow(r)}});",
            "}});",
            "}}",
            "html+='</div>';",
            "if(D.merged_content){{",
            "html+='<div class=\"__bb_mg\">';",
            "html+='<div class=\"__bb_mt\" onclick=\"var c=this.nextElementSibling;c.classList.toggle(\\x27--open\\x27);this.querySelector(\\x27span\\x27).textContent=c.classList.contains(\\x27--open\\x27)?\\x27\\u25B4\\x27:\\x27\\u25BE\\x27\"><span>\\u25BE</span> Merged Rules</div>';",
            "html+='<div class=\"__bb_mcw\"><button type=\"button\" class=\"__bb_copy\" title=\"Copy merged rules\" aria-label=\"Copy merged rules\">Copy</button><div class=\"__bb_mc\">'+esc(D.merged_content)+'</div></div>';",
            "html+='</div>';",
            "}}",
            "P.innerHTML=html;",
            "Array.prototype.forEach.call(P.querySelectorAll('.__bb_copy'),function(btn){{btn.onclick=function(ev){{ev.stopPropagation();copyMerged(btn)}}}});",
            "}}",
            "B.onmouseenter=show;",
            "B.onmouseleave=hide;",
            "P.onmouseenter=function(){{clearTimeout(hideTimer)}};",
            "P.onmouseleave=hide;",
            "B.onclick=function(){{B.style.display='none';P.classList.remove('--visible')}};",
            "}})();",
            "</script>",
        ),
        rules_json = rules_json,
    )
}

fn build_badge_snippet(rules_json: &str) -> String {
    let mut s = String::with_capacity(4096);
    s.push_str(BADGE_STYLE);
    s.push_str(BADGE_HTML);
    s.push_str(&badge_script(rules_json));
    s
}

fn contains_badge(body: &[u8]) -> bool {
    let marker = BIFROST_BADGE_ELEMENT_ID.as_bytes();
    body.windows(marker.len()).any(|w| w == marker)
}

fn find_last_body_close_tag_start(body: &[u8]) -> Option<usize> {
    const PATTERN: &[u8] = b"</body>";
    if body.len() < PATTERN.len() {
        return None;
    }

    for start in (0..=body.len() - PATTERN.len()).rev() {
        if body[start..start + PATTERN.len()]
            .iter()
            .zip(PATTERN.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(start);
        }
    }
    None
}

pub fn maybe_inject_bifrost_badge_html(body: Bytes, rules_json: &str) -> (Bytes, bool) {
    if body.is_empty() || contains_badge(&body) {
        return (body, false);
    }

    let snippet = build_badge_snippet(rules_json);
    let snippet_bytes = snippet.as_bytes();

    if let Some(insert_at) = find_last_body_close_tag_start(&body) {
        let mut out = Vec::with_capacity(body.len() + snippet_bytes.len());
        out.extend_from_slice(&body[..insert_at]);
        out.extend_from_slice(snippet_bytes);
        out.extend_from_slice(&body[insert_at..]);
        (Bytes::from(out), true)
    } else {
        let mut out = Vec::with_capacity(body.len() + snippet_bytes.len());
        out.extend_from_slice(&body);
        out.extend_from_slice(snippet_bytes);
        (Bytes::from(out), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_RULES: &str = r#"{"rules":[],"merged_content":"","admin_port":8800}"#;
    const SAMPLE_RULES: &str = r#"{"rules":[{"name":"my-rule","rule_count":3,"group_id":null,"group_name":null}],"merged_content":"example.com mock 200","admin_port":8800}"#;

    #[test]
    fn test_inject_badge_before_body_end() {
        let html = Bytes::from_static(b"<html><body>Hello</body></html>");
        let (out, changed) = maybe_inject_bifrost_badge_html(html, EMPTY_RULES);
        assert!(changed);

        let out_str = String::from_utf8(out.to_vec()).unwrap();
        let badge_pos = out_str.find(BIFROST_BADGE_ELEMENT_ID).unwrap();
        let body_close_pos = out_str.to_ascii_lowercase().rfind("</body>").unwrap();
        assert!(badge_pos < body_close_pos);
    }

    #[test]
    fn test_inject_badge_append_when_no_body_end() {
        let html = Bytes::from_static(b"<html>Hello</html>");
        let (out, changed) = maybe_inject_bifrost_badge_html(html, EMPTY_RULES);
        assert!(changed);

        let out_str = String::from_utf8(out.to_vec()).unwrap();
        assert!(out_str.starts_with("<html>Hello</html>"));
        assert!(out_str.contains(BIFROST_BADGE_ELEMENT_ID));
    }

    #[test]
    fn test_badge_contains_b_character_and_click_hide() {
        let snippet = build_badge_snippet(EMPTY_RULES);
        assert!(snippet.contains("__bb_ico"));
        assert!(snippet.contains(">B</span>"));
        assert!(snippet.contains("cursor:pointer"));
        assert!(snippet.contains(":hover"));
        assert!(snippet.contains("left:15px"));
        assert!(snippet.contains("bottom:15px"));
    }

    #[test]
    fn test_inject_badge_case_insensitive_body_end() {
        let html = Bytes::from_static(b"<html><body>Hello</BODY></html>");
        let (out, changed) = maybe_inject_bifrost_badge_html(html, EMPTY_RULES);
        assert!(changed);

        let out_str = String::from_utf8(out.to_vec()).unwrap();
        let badge_pos = out_str.find(BIFROST_BADGE_ELEMENT_ID).unwrap();
        let body_close_pos = out_str.to_ascii_lowercase().rfind("</body>").unwrap();
        assert!(badge_pos < body_close_pos);
    }

    #[test]
    fn test_badge_snippet_contains_inline_rules_data() {
        let snippet = build_badge_snippet(SAMPLE_RULES);
        assert!(snippet.contains("my-rule"));
        assert!(snippet.contains("rule_count"));
        assert!(snippet.contains("merged_content"));
        assert!(snippet.contains("admin_port"));
        assert!(!snippet.contains("fetch("));
    }

    #[test]
    fn test_badge_panel_html_present() {
        let snippet = build_badge_snippet(EMPTY_RULES);
        assert!(snippet.contains("__bb_panel__"));
        assert!(snippet.contains("--visible"));
        assert!(snippet.contains("onmouseenter"));
        assert!(snippet.contains("onmouseleave"));
    }

    #[test]
    fn test_badge_panel_uses_top_z_index() {
        let snippet = build_badge_snippet(EMPTY_RULES);
        assert!(snippet.contains(
            "#__bifrost_badge__{position:fixed;left:15px;bottom:15px;z-index:2147483647!important;"
        ));
        assert!(snippet.contains(
            "#__bb_panel__{position:fixed;left:15px;bottom:52px;z-index:2147483647!important;"
        ));
    }

    #[test]
    fn test_badge_merged_rules_copy_button_present() {
        let snippet = build_badge_snippet(SAMPLE_RULES);
        assert!(snippet.contains("__bb_copy"));
        assert!(snippet.contains("Copy merged rules"));
        assert!(snippet.contains("navigator.clipboard"));
        assert!(snippet.contains("document.execCommand('copy')"));
        assert!(snippet.contains("clipboardData.setData('text/plain',text)"));
        assert!(snippet.contains("ok&&copied?resolve()"));
        assert!(snippet.contains("copyMerged(btn)"));
    }

    #[test]
    fn test_skip_duplicate_injection() {
        let html = Bytes::from_static(b"<html><body>Hello</body></html>");
        let (out, changed) = maybe_inject_bifrost_badge_html(html, EMPTY_RULES);
        assert!(changed);

        let (out2, changed2) = maybe_inject_bifrost_badge_html(out, EMPTY_RULES);
        assert!(!changed2);
        let _ = out2;
    }

    #[test]
    fn test_badge_rule_row_links_to_admin_ui() {
        let snippet = build_badge_snippet(SAMPLE_RULES);
        assert!(snippet.contains("target="));
        assert!(snippet.contains("_blank"));
        assert!(snippet.contains("/_bifrost/rules"));
        assert!(snippet.contains("?rule="));
    }
}
