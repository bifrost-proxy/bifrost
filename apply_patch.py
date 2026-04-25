#!/usr/bin/env python3
"""Apply fire-and-forget patch to worker.rs L2460-2490.

Wraps the serial-await post_call_stream_frame in tokio::spawn so the per-chunk
sink keeps only the legacy /frame on its critical path. Failures on the new
stream_frame path are still logged.
"""
import re
import sys
from pathlib import Path

path = Path("crates/bifrost-admin/src/remote_invoke/worker.rs")
src = path.read_text()

old = """                        let legacy_result = relay_client.post_call_frame(&cid, &frame_req).await;
                        // PR#6c: parallel emission to the new /stream-frame
                        // endpoint. Offset is left at 0 for now; a follow-up
                        // PR will thread the pre-tee absolute head in. Failure
                        // on this path is swallowed so it never breaks the
                        // legacy envelope path (which `legacy_result` tracks).
                        let stream_frame = stream_emit::build_stdout_frame(
                            seq,
                            offset_for_stream,
                            &chunk_bytes_for_stream,
                        );
                        if let Some(frame_json) = stream_emit::frame_to_json(&stream_frame) {
                            let stream_req = ClientCallStreamFrameRequest {
                                call_id: cid.clone(),
                                client_instance_id: instance_id_for_stream,
                                frame_json,
                            };
                            if let Err(err) =
                                relay_client.post_call_stream_frame(&cid, &stream_req).await
                            {
                                tracing::debug!(
                                    call_id = %cid,
                                    ?err,
                                    "parallel stream_frame post failed"
                                );
                            }
                        }
                        legacy_result"""

new = """                        let legacy_result = relay_client.post_call_frame(&cid, &frame_req).await;
                        // PR#6c (fire-and-forget): the new /stream-frame
                        // emission is intentionally detached so its latency
                        // (and relay rate-limit/429 retries) cannot add to
                        // the per-chunk critical path. Under CI load, the
                        // prior serial `.await` on both endpoints pushed
                        // single-test wall time past the 900s orchestrator
                        // budget and caused `remote search` / `traffic
                        // search` to hang. Failures are still logged; the
                        // legacy envelope path (tracked by `legacy_result`)
                        // remains the source of truth.
                        let stream_frame = stream_emit::build_stdout_frame(
                            seq,
                            offset_for_stream,
                            &chunk_bytes_for_stream,
                        );
                        if let Some(frame_json) = stream_emit::frame_to_json(&stream_frame) {
                            let stream_req = ClientCallStreamFrameRequest {
                                call_id: cid.clone(),
                                client_instance_id: instance_id_for_stream,
                                frame_json,
                            };
                            let relay_client_for_stream = Arc::clone(&relay_client);
                            let cid_for_stream = cid.clone();
                            tokio::spawn(async move {
                                if let Err(err) = relay_client_for_stream
                                    .post_call_stream_frame(&cid_for_stream, &stream_req)
                                    .await
                                {
                                    tracing::debug!(
                                        call_id = %cid_for_stream,
                                        ?err,
                                        "parallel stream_frame post failed"
                                    );
                                }
                            });
                        }
                        legacy_result"""

if old not in src:
    print("ERROR: exact block not found; aborting", file=sys.stderr)
    sys.exit(1)

count = src.count(old)
if count != 1:
    print(f"ERROR: expected exactly 1 occurrence, found {count}", file=sys.stderr)
    sys.exit(1)

path.write_text(src.replace(old, new))
print("OK: patch applied")
