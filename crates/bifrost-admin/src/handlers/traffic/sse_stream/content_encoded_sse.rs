use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::super::body::configured_decompress_output_bytes;
use super::*;
use crate::handlers::network_body::content_encoding_is_supported;

#[derive(Default)]
struct DecoderOutput {
    bytes: Vec<u8>,
}

#[derive(Default)]
struct DeflateProbeOutput {
    bytes: Vec<u8>,
    limit_exceeded: bool,
}

struct OutputWriter(Arc<Mutex<DecoderOutput>>);

trait DecodeWriter: Write + Send + Sync {
    fn finish_decode(&mut self) -> io::Result<()>;
}

impl Write for OutputWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl DecodeWriter for OutputWriter {
    fn finish_decode(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct DeflateProbeWriter(Arc<Mutex<DeflateProbeOutput>>);

impl Write for DeflateProbeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut output = self.0.lock().unwrap();
        if buf.len() > MAX_DEFLATE_PROBE_OUTPUT_BYTES.saturating_sub(output.bytes.len()) {
            output.limit_exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "deflate probe output limit exceeded",
            ));
        }
        output.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl DecodeWriter for DeflateProbeWriter {
    fn finish_decode(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct DecodeBudget {
    used: usize,
    max: usize,
    exceeded: bool,
}

struct BudgetWriter {
    inner: Box<dyn DecodeWriter>,
    budget: Arc<Mutex<DecodeBudget>>,
}

impl DecodeWriter for BudgetWriter {
    fn finish_decode(&mut self) -> io::Result<()> {
        self.inner.finish_decode()
    }
}

struct GzipDecodeWriter(flate2::write::MultiGzDecoder<Box<dyn DecodeWriter>>);

impl Write for GzipDecodeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl DecodeWriter for GzipDecodeWriter {
    fn finish_decode(&mut self) -> io::Result<()> {
        self.0.try_finish()?;
        self.0.get_mut().finish_decode()
    }
}

struct BrotliDecodeWriter(brotli::DecompressorWriter<Box<dyn DecodeWriter>>);

impl Write for BrotliDecodeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl DecodeWriter for BrotliDecodeWriter {
    fn finish_decode(&mut self) -> io::Result<()> {
        self.0.close()?;
        self.0.get_mut().finish_decode()
    }
}

struct ZstdDecodeWriter(
    zstd::stream::zio::Writer<Box<dyn DecodeWriter>, zstd::stream::raw::Decoder<'static>>,
);

impl Write for ZstdDecodeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl DecodeWriter for ZstdDecodeWriter {
    fn finish_decode(&mut self) -> io::Result<()> {
        self.0.finish()?;
        self.0.writer_mut().finish_decode()
    }
}

const MAX_DEFLATE_PROBE_INPUT_BYTES: usize = 64 * 1024;
const MAX_DEFLATE_PROBE_OUTPUT_BYTES: usize = 8 * 1024;

impl Write for BudgetWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        {
            let mut budget = self.budget.lock().unwrap();
            if buf.len() > budget.max.saturating_sub(budget.used) {
                budget.exceeded = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "decompressed body exceeds the preview limit",
                ));
            }
            budget.used = budget.used.saturating_add(buf.len());
        }
        self.inner.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

enum DeflateDecoderState {
    Pending {
        downstream: Option<Box<dyn DecodeWriter>>,
        prefix: Vec<u8>,
    },
    ZlibProbe {
        decoder: flate2::write::ZlibDecoder<Box<dyn DecodeWriter>>,
        downstream: Option<Box<dyn DecodeWriter>>,
        input: Vec<u8>,
        output: Arc<Mutex<DeflateProbeOutput>>,
    },
    Zlib(flate2::write::ZlibDecoder<Box<dyn DecodeWriter>>),
    Raw(flate2::write::DeflateDecoder<Box<dyn DecodeWriter>>),
}

struct StreamingDeflateDecoder {
    state: DeflateDecoderState,
}

impl StreamingDeflateDecoder {
    fn new(downstream: Box<dyn DecodeWriter>) -> Self {
        Self {
            state: DeflateDecoderState::Pending {
                downstream: Some(downstream),
                prefix: Vec::with_capacity(2),
            },
        }
    }

    fn initialize_if_ready(&mut self) -> io::Result<()> {
        let DeflateDecoderState::Pending { downstream, prefix } = &mut self.state else {
            return Ok(());
        };
        if prefix.len() < 2 {
            return Ok(());
        }

        let is_zlib = prefix[0] & 0x0f == 8
            && prefix[0] >> 4 <= 7
            && ((u16::from(prefix[0]) << 8) | u16::from(prefix[1])) % 31 == 0;
        let downstream = downstream.take().expect("deflate downstream is present");
        let initial = std::mem::take(prefix);
        if is_zlib {
            let output = Arc::new(Mutex::new(DeflateProbeOutput::default()));
            let mut decoder = flate2::write::ZlibDecoder::new(Box::new(DeflateProbeWriter(
                output.clone(),
            ))
                as Box<dyn DecodeWriter>);
            decoder.write_all(&initial)?;
            self.state = DeflateDecoderState::ZlibProbe {
                decoder,
                downstream: Some(downstream),
                input: initial,
                output,
            };
        } else {
            let mut decoder = flate2::write::DeflateDecoder::new(downstream);
            decoder.write_all(&initial)?;
            self.state = DeflateDecoderState::Raw(decoder);
        }
        Ok(())
    }
}

impl Write for StreamingDeflateDecoder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let DeflateDecoderState::Pending { prefix, .. } = &mut self.state {
            let prefix_bytes = (2 - prefix.len()).min(buf.len());
            prefix.extend_from_slice(&buf[..prefix_bytes]);
            self.initialize_if_ready()?;
            if prefix_bytes < buf.len() {
                self.write_all(&buf[prefix_bytes..])?;
            }
            return Ok(buf.len());
        }
        if matches!(self.state, DeflateDecoderState::ZlibProbe { .. }) {
            let placeholder = DeflateDecoderState::Pending {
                downstream: None,
                prefix: Vec::new(),
            };
            let DeflateDecoderState::ZlibProbe {
                mut decoder,
                mut downstream,
                mut input,
                output,
            } = std::mem::replace(&mut self.state, placeholder)
            else {
                unreachable!();
            };
            let exceeds_probe_limit =
                buf.len() > MAX_DEFLATE_PROBE_INPUT_BYTES.saturating_sub(input.len());
            let decode_result = decoder.write_all(buf);
            let probe = output.lock().unwrap();
            let probe_has_output = !probe.bytes.is_empty();
            let probe_limit_exceeded = probe.limit_exceeded;
            drop(probe);
            if decode_result.is_err() && !probe_limit_exceeded {
                let mut raw = flate2::write::DeflateDecoder::new(
                    downstream
                        .take()
                        .expect("deflate probe downstream is present"),
                );
                raw.write_all(&input)?;
                raw.write_all(buf)?;
                self.state = DeflateDecoderState::Raw(raw);
            } else if exceeds_probe_limit || probe_has_output || probe_limit_exceeded {
                let mut zlib = flate2::write::ZlibDecoder::new(
                    downstream
                        .take()
                        .expect("deflate probe downstream is present"),
                );
                zlib.write_all(&input)?;
                zlib.write_all(buf)?;
                self.state = DeflateDecoderState::Zlib(zlib);
            } else {
                input.extend_from_slice(buf);
                self.state = DeflateDecoderState::ZlibProbe {
                    decoder,
                    downstream,
                    input,
                    output,
                };
            }
            return Ok(buf.len());
        }
        match &mut self.state {
            DeflateDecoderState::Zlib(decoder) => decoder.write(buf),
            DeflateDecoderState::Raw(decoder) => decoder.write(buf),
            DeflateDecoderState::Pending { .. } | DeflateDecoderState::ZlibProbe { .. } => {
                unreachable!()
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.state {
            DeflateDecoderState::Pending { downstream, .. } => {
                if let Some(downstream) = downstream {
                    downstream.flush()
                } else {
                    Ok(())
                }
            }
            DeflateDecoderState::ZlibProbe { .. } => {
                let placeholder = DeflateDecoderState::Pending {
                    downstream: None,
                    prefix: Vec::new(),
                };
                let DeflateDecoderState::ZlibProbe {
                    mut decoder,
                    mut downstream,
                    input,
                    output,
                } = std::mem::replace(&mut self.state, placeholder)
                else {
                    unreachable!();
                };
                if decoder.flush().is_err() {
                    let mut raw = flate2::write::DeflateDecoder::new(
                        downstream
                            .take()
                            .expect("deflate probe downstream is present"),
                    );
                    raw.write_all(&input)?;
                    raw.flush()?;
                    self.state = DeflateDecoderState::Raw(raw);
                } else if !output.lock().unwrap().bytes.is_empty() {
                    let mut zlib = flate2::write::ZlibDecoder::new(
                        downstream
                            .take()
                            .expect("deflate probe downstream is present"),
                    );
                    zlib.write_all(&input)?;
                    zlib.flush()?;
                    self.state = DeflateDecoderState::Zlib(zlib);
                } else {
                    self.state = DeflateDecoderState::ZlibProbe {
                        decoder,
                        downstream,
                        input,
                        output,
                    };
                }
                Ok(())
            }
            DeflateDecoderState::Zlib(decoder) => decoder.flush(),
            DeflateDecoderState::Raw(decoder) => decoder.flush(),
        }
    }
}

impl DecodeWriter for StreamingDeflateDecoder {
    fn finish_decode(&mut self) -> io::Result<()> {
        if let DeflateDecoderState::Pending { prefix, .. } = &self.state {
            if prefix.len() < 2 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated deflate stream",
                ));
            }
        }
        self.initialize_if_ready()?;

        if matches!(self.state, DeflateDecoderState::ZlibProbe { .. }) {
            let placeholder = DeflateDecoderState::Pending {
                downstream: None,
                prefix: Vec::new(),
            };
            let DeflateDecoderState::ZlibProbe {
                mut decoder,
                downstream,
                input,
                output,
            } = std::mem::replace(&mut self.state, placeholder)
            else {
                unreachable!();
            };
            let mut downstream = downstream.expect("deflate probe downstream is present");
            if decoder.try_finish().is_ok() {
                let decoded = std::mem::take(&mut output.lock().unwrap().bytes);
                downstream.write_all(&decoded)?;
                return downstream.finish_decode();
            }

            let mut raw = flate2::write::DeflateDecoder::new(downstream);
            raw.write_all(&input)?;
            raw.try_finish()?;
            return raw.get_mut().finish_decode();
        }

        match &mut self.state {
            DeflateDecoderState::Zlib(decoder) => {
                decoder.try_finish()?;
                decoder.get_mut().finish_decode()
            }
            DeflateDecoderState::Raw(decoder) => {
                decoder.try_finish()?;
                decoder.get_mut().finish_decode()
            }
            DeflateDecoderState::Pending { .. } | DeflateDecoderState::ZlibProbe { .. } => {
                unreachable!()
            }
        }
    }
}

pub struct IncrementalContentDecoder {
    writer: Box<dyn DecodeWriter>,
    output: Arc<Mutex<DecoderOutput>>,
    budget: Arc<Mutex<DecodeBudget>>,
    wire_prefix: WirePrefixValidator,
}

enum WirePrefixValidator {
    None,
    Gzip(Vec<u8>),
    Zstd(Vec<u8>),
}

impl WirePrefixValidator {
    fn for_content_encoding(content_encoding: &str) -> Self {
        let outermost = content_encoding
            .split(',')
            .map(str::trim)
            .rfind(|encoding| !encoding.is_empty() && !encoding.eq_ignore_ascii_case("identity"));
        match outermost.map(str::to_ascii_lowercase).as_deref() {
            Some("gzip" | "x-gzip") => Self::Gzip(Vec::with_capacity(2)),
            Some("zstd") => Self::Zstd(Vec::with_capacity(4)),
            _ => Self::None,
        }
    }

    fn validate(&mut self, wire_bytes: &[u8]) -> io::Result<Option<Vec<u8>>> {
        let (prefix, required) = match self {
            Self::None => return Ok(Some(wire_bytes.to_vec())),
            Self::Gzip(prefix) => (prefix, 2),
            Self::Zstd(prefix) => (prefix, 4),
        };
        prefix.extend_from_slice(wire_bytes);
        if prefix.len() < required {
            return Ok(None);
        }

        let valid = match self {
            Self::Gzip(prefix) => prefix.starts_with(&[0x1f, 0x8b]),
            Self::Zstd(prefix) => {
                prefix.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
                    || (prefix[0] & 0xf0 == 0x50 && prefix[1..4] == [0x2a, 0x4d, 0x18])
            }
            Self::None => unreachable!(),
        };
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid content-encoding frame header",
            ));
        }

        let buffered = match std::mem::replace(self, Self::None) {
            Self::Gzip(prefix) | Self::Zstd(prefix) => prefix,
            Self::None => unreachable!(),
        };
        Ok(Some(buffered))
    }
}

impl IncrementalContentDecoder {
    pub fn new(content_encoding: &str, max_output_bytes: usize) -> io::Result<Self> {
        let output = Arc::new(Mutex::new(DecoderOutput::default()));
        let budget = Arc::new(Mutex::new(DecodeBudget {
            used: 0,
            max: max_output_bytes,
            exceeded: false,
        }));
        let mut writer: Box<dyn DecodeWriter> = Box::new(OutputWriter(output.clone()));

        for encoding in content_encoding
            .split(',')
            .map(str::trim)
            .filter(|encoding| !encoding.is_empty())
        {
            if encoding.eq_ignore_ascii_case("identity") {
                continue;
            }
            let downstream: Box<dyn DecodeWriter> = Box::new(BudgetWriter {
                inner: writer,
                budget: budget.clone(),
            });
            writer = match encoding.to_ascii_lowercase().as_str() {
                "gzip" | "x-gzip" => Box::new(GzipDecodeWriter(
                    flate2::write::MultiGzDecoder::new(downstream),
                )),
                "deflate" => Box::new(StreamingDeflateDecoder::new(downstream)),
                "br" => Box::new(BrotliDecodeWriter(brotli::DecompressorWriter::new(
                    downstream, 4096,
                ))),
                "zstd" => Box::new(ZstdDecodeWriter(zstd::stream::zio::Writer::new(
                    downstream,
                    zstd::stream::raw::Decoder::new()?,
                ))),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("unsupported content-encoding: {encoding}"),
                    ));
                }
            };
        }

        Ok(Self {
            writer,
            output,
            budget,
            wire_prefix: WirePrefixValidator::for_content_encoding(content_encoding),
        })
    }

    pub fn push(&mut self, wire_bytes: &[u8]) -> io::Result<Vec<u8>> {
        let Some(wire_bytes) = self.wire_prefix.validate(wire_bytes)? else {
            return Ok(Vec::new());
        };
        self.writer.write_all(&wire_bytes)?;
        self.writer.flush()?;
        Ok(std::mem::take(&mut self.output.lock().unwrap().bytes))
    }

    pub fn take_output(&self) -> Vec<u8> {
        std::mem::take(&mut self.output.lock().unwrap().bytes)
    }

    pub fn finish(&mut self) -> io::Result<Vec<u8>> {
        self.writer.finish_decode()?;
        Ok(self.take_output())
    }

    pub fn exceeded_limit(&self) -> bool {
        self.budget.lock().unwrap().exceeded
    }
}

async fn emit_decoded_events(
    decoded: &[u8],
    parser: &mut SseIncrementalParser,
    seq: &mut u64,
    batch: &mut Vec<crate::sse::SseEvent>,
    batch_size: usize,
    last_event_was_finish: &mut bool,
    tx: &tokio::sync::mpsc::Sender<bytes::Bytes>,
) -> Result<(), ()> {
    let mut produced = Vec::new();
    parser.push_bytes(decoded, &mut produced);
    for raw in produced {
        *seq = seq.saturating_add(1);
        let event = sse_event_from_raw(*seq, now_ms(), raw);
        *last_event_was_finish = event.event.as_deref() == Some("finish");
        if batch_size <= 1 {
            if tx
                .send(bytes::Bytes::from(sse_json_line(&event)))
                .await
                .is_err()
            {
                return Err(());
            }
        } else {
            batch.push(event);
            if batch.len() >= batch_size {
                let payload = sse_json_batch_line(batch);
                batch.clear();
                if tx.send(bytes::Bytes::from(payload)).await.is_err() {
                    return Err(());
                }
            }
        }
    }
    Ok(())
}

async fn emit_limit_error(
    max_output_bytes: usize,
    seq: &mut u64,
    batch: &mut Vec<crate::sse::SseEvent>,
    batch_size: usize,
    tx: &tokio::sync::mpsc::Sender<bytes::Bytes>,
) {
    *seq = seq.saturating_add(1);
    let error_event = crate::sse::SseEvent {
        seq: *seq,
        ts: now_ms(),
        id: None,
        event: Some("error".to_string()),
        retry: None,
        data: format!(
            "decoded SSE body exceeds the configured {} byte limit",
            max_output_bytes
        ),
        raw: None,
        parse_error: true,
    };
    if batch_size <= 1 {
        let _ = tx
            .send(bytes::Bytes::from(sse_json_line(&error_event)))
            .await;
    } else {
        batch.push(error_event);
        let _ = tx
            .send(bytes::Bytes::from(sse_json_batch_line(batch)))
            .await;
    }
}

async fn emit_decode_error(
    seq: &mut u64,
    batch: &mut Vec<crate::sse::SseEvent>,
    batch_size: usize,
    tx: &tokio::sync::mpsc::Sender<bytes::Bytes>,
) {
    *seq = seq.saturating_add(1);
    let error_event = crate::sse::SseEvent {
        seq: *seq,
        ts: now_ms(),
        id: None,
        event: Some("error".to_string()),
        retry: None,
        data: "failed to decode content-encoded SSE body".to_string(),
        raw: None,
        parse_error: true,
    };
    if batch_size <= 1 {
        let _ = tx
            .send(bytes::Bytes::from(sse_json_line(&error_event)))
            .await;
    } else {
        batch.push(error_event);
        let _ = tx
            .send(bytes::Bytes::from(sse_json_batch_line(batch)))
            .await;
    }
}

pub(super) async fn stream_content_encoded_sse_events(
    state: SharedAdminState,
    connection_id: &str,
    body_ref: BodyRef,
    from: SseStreamFrom,
    batch_size: usize,
    tail_bytes: usize,
    tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
) -> Result<(), ()> {
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    use tokio::time::{sleep, Duration};

    let max_output_bytes = configured_decompress_output_bytes(&state).await;
    let Some(content_encoding) = body_ref.content_encoding() else {
        return Ok(());
    };
    if !content_encoding_is_supported(&content_encoding) {
        return Ok(());
    }

    let (path, start_offset, fixed_end) = match body_ref {
        BodyRef::File { path, .. } => (path, 0, None),
        BodyRef::FileRange {
            path, offset, size, ..
        } => (path, offset, Some(offset.saturating_add(size as u64))),
        BodyRef::Inline { .. } => return Ok(()),
    };
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => return Ok(()),
    };
    if file
        .seek(std::io::SeekFrom::Start(start_offset))
        .await
        .is_err()
    {
        return Ok(());
    }

    let mut decoder = match IncrementalContentDecoder::new(&content_encoding, max_output_bytes) {
        Ok(decoder) => decoder,
        Err(_) => return Ok(()),
    };
    let mut parser = SseIncrementalParser::new();
    let mut seq = 0u64;
    let mut batch = Vec::new();
    let batch_size = batch_size.max(1);
    let mut last_force_flush_refresh = Instant::now();
    let mut closed_eof_retries = 0u8;
    let mut saw_closed = false;
    let mut last_event_was_finish = false;
    let mut offset = start_offset;
    let mut buf = vec![0u8; 8192];
    let mut tail_pending = (from == SseStreamFrom::Tail).then(Vec::new);
    loop {
        if last_force_flush_refresh.elapsed() >= Duration::from_secs(5) {
            state.sse_hub.request_force_flush(connection_id, 30_000);
            last_force_flush_refresh = Instant::now();
        }

        let is_open = state.sse_hub.is_open(connection_id).unwrap_or(false);
        if !is_open {
            saw_closed = true;
        }
        if let Some(end) = fixed_end {
            if offset >= end {
                break;
            }
        }

        let to_read = fixed_end
            .map(|end| (end.saturating_sub(offset) as usize).min(buf.len()))
            .unwrap_or(buf.len());
        let read = if to_read == 0 {
            0
        } else {
            file.read(&mut buf[..to_read]).await.unwrap_or_default()
        };

        if read > 0 {
            closed_eof_retries = 0;
            offset = offset.saturating_add(read as u64);
            match decoder.push(&buf[..read]) {
                Ok(decoded) => {
                    if let Some(pending) = &mut tail_pending {
                        pending.extend_from_slice(&decoded);
                    } else if emit_decoded_events(
                        &decoded,
                        &mut parser,
                        &mut seq,
                        &mut batch,
                        batch_size,
                        &mut last_event_was_finish,
                        &tx,
                    )
                    .await
                    .is_err()
                    {
                        return Ok(());
                    }
                }
                Err(_) if decoder.exceeded_limit() => {
                    emit_limit_error(max_output_bytes, &mut seq, &mut batch, batch_size, &tx).await;
                    return Ok(());
                }
                Err(_) => {
                    emit_decode_error(&mut seq, &mut batch, batch_size, &tx).await;
                    return Ok(());
                }
            }
            continue;
        }

        let trailing = decoder.take_output();
        if !trailing.is_empty() {
            if let Some(pending) = &mut tail_pending {
                pending.extend_from_slice(&trailing);
            } else if emit_decoded_events(
                &trailing,
                &mut parser,
                &mut seq,
                &mut batch,
                batch_size,
                &mut last_event_was_finish,
                &tx,
            )
            .await
            .is_err()
            {
                return Ok(());
            }
        }

        if let Some(pending) = tail_pending.take() {
            let start = pending.len().saturating_sub(tail_bytes);
            if emit_decoded_events(
                &pending[start..],
                &mut parser,
                &mut seq,
                &mut batch,
                batch_size,
                &mut last_event_was_finish,
                &tx,
            )
            .await
            .is_err()
            {
                return Ok(());
            }
        }

        if fixed_end.is_some() {
            break;
        }
        if !is_open {
            closed_eof_retries = closed_eof_retries.saturating_add(1);
            if closed_eof_retries >= 10 {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        } else {
            closed_eof_retries = 0;
            sleep(Duration::from_millis(100)).await;
        }
    }

    match decoder.finish() {
        Ok(trailing) => {
            if !trailing.is_empty()
                && emit_decoded_events(
                    &trailing,
                    &mut parser,
                    &mut seq,
                    &mut batch,
                    batch_size,
                    &mut last_event_was_finish,
                    &tx,
                )
                .await
                .is_err()
            {
                return Ok(());
            }
        }
        Err(_) if decoder.exceeded_limit() => {
            emit_limit_error(max_output_bytes, &mut seq, &mut batch, batch_size, &tx).await;
            return Ok(());
        }
        Err(_) => {
            emit_decode_error(&mut seq, &mut batch, batch_size, &tx).await;
            return Ok(());
        }
    }

    if let Some(raw) = parser.finish() {
        seq = seq.saturating_add(1);
        let event = sse_event_from_raw(seq, now_ms(), raw);
        last_event_was_finish = event.event.as_deref() == Some("finish");
        if batch_size <= 1 {
            let _ = tx.send(bytes::Bytes::from(sse_json_line(&event))).await;
        } else {
            batch.push(event);
        }
    }

    if should_emit_synthetic_finish(fixed_end, saw_closed, last_event_was_finish) {
        seq = seq.saturating_add(1);
        let finish_event = crate::sse::SseEvent {
            seq,
            ts: now_ms(),
            id: None,
            event: Some("finish".to_string()),
            retry: None,
            data: String::new(),
            raw: None,
            parse_error: false,
        };
        if batch_size <= 1 {
            let _ = tx
                .send(bytes::Bytes::from(sse_json_line(&finish_event)))
                .await;
        } else {
            batch.push(finish_event);
        }
    }
    if batch_size > 1 && !batch.is_empty() {
        let _ = tx
            .send(bytes::Bytes::from(sse_json_batch_line(&batch)))
            .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{
        emit_decode_error, emit_decoded_events, stream_content_encoded_sse_events, DecodeWriter,
        DecoderOutput, DeflateDecoderState, DeflateProbeOutput, DeflateProbeWriter,
        IncrementalContentDecoder, OutputWriter, StreamingDeflateDecoder,
        MAX_DEFLATE_PROBE_INPUT_BYTES, MAX_DEFLATE_PROBE_OUTPUT_BYTES,
    };
    use crate::handlers::traffic::sse_stream::{SseIncrementalParser, SseStreamFrom};
    use crate::{AdminState, BodyRef};
    use std::sync::{Arc, Mutex};

    fn decode_in_small_wire_chunks(encoding: &str, wire: &[u8]) -> Vec<u8> {
        let mut decoder = IncrementalContentDecoder::new(encoding, 1024).unwrap();
        let mut decoded = Vec::new();
        for chunk in wire.chunks(3) {
            decoded.extend(decoder.push(chunk).unwrap());
        }
        decoded.extend(decoder.finish().unwrap());
        decoded
    }

    #[test]
    fn incremental_decoder_supports_all_standard_stream_codings() {
        let plaintext = b"data: standard coding\n\n";

        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(plaintext).unwrap();
        assert_eq!(
            decode_in_small_wire_chunks("gzip", &gzip.finish().unwrap()),
            plaintext
        );

        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(plaintext).unwrap();
        assert_eq!(
            decode_in_small_wire_chunks("deflate", &zlib.finish().unwrap()),
            plaintext
        );

        let mut raw =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw.write_all(plaintext).unwrap();
        assert_eq!(
            decode_in_small_wire_chunks("deflate", &raw.finish().unwrap()),
            plaintext
        );

        let mut br = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
            encoder.write_all(plaintext).unwrap();
        }
        assert_eq!(decode_in_small_wire_chunks("br", &br), plaintext);

        let zstd = zstd::stream::encode_all(plaintext.as_slice(), 1).unwrap();
        assert_eq!(decode_in_small_wire_chunks("zstd", &zstd), plaintext);
    }

    #[test]
    fn streaming_deflate_handles_short_prefix_flush_and_initialized_writes() {
        let plaintext = b"data: split deflate prefix\n\n";
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(plaintext).unwrap();
        let wire = encoder.finish().unwrap();
        let output = Arc::new(Mutex::new(DecoderOutput::default()));
        let mut decoder = StreamingDeflateDecoder::new(Box::new(OutputWriter(output.clone())));

        decoder.initialize_if_ready().unwrap();
        decoder.write_all(&wire[..1]).unwrap();
        decoder.flush().unwrap();
        decoder.write_all(&wire[1..2]).unwrap();
        decoder.initialize_if_ready().unwrap();
        decoder.write_all(&wire[2..]).unwrap();
        decoder.flush().unwrap();

        assert_eq!(output.lock().unwrap().bytes, plaintext);

        let mut empty = StreamingDeflateDecoder {
            state: DeflateDecoderState::Pending {
                downstream: None,
                prefix: Vec::new(),
            },
        };
        empty.flush().unwrap();
    }

    #[test]
    fn streaming_deflate_retries_raw_after_false_zlib_header_match() {
        let plaintext = b"data: raw false zlib match!\n\n";
        assert_eq!(plaintext.len(), 29);
        // A raw stored block whose first two bytes also satisfy the zlib header
        // checksum: 0x081d % 31 == 0.
        let mut wire = vec![0x08, 0x1d, 0x00, 0xe2, 0xff];
        wire.extend_from_slice(plaintext);
        wire.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
        let output = Arc::new(Mutex::new(DecoderOutput::default()));
        let mut decoder = StreamingDeflateDecoder::new(Box::new(OutputWriter(output.clone())));

        decoder.write_all(&wire[..2]).unwrap();
        decoder.flush().unwrap();
        decoder.write_all(&wire[2..]).unwrap();
        decoder.flush().unwrap();

        assert_eq!(output.lock().unwrap().bytes, plaintext);
    }

    #[test]
    fn streaming_deflate_selects_zlib_or_raw_from_a_complete_first_chunk() {
        let plaintext = b"data: complete first chunk!\n\n";
        let mut zlib_encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib_encoder.write_all(plaintext).unwrap();
        let zlib_wire = zlib_encoder.finish().unwrap();
        let zlib_output = Arc::new(Mutex::new(DecoderOutput::default()));
        let mut zlib = StreamingDeflateDecoder::new(Box::new(OutputWriter(zlib_output.clone())));
        zlib.write_all(&zlib_wire).unwrap();
        zlib.flush().unwrap();
        assert_eq!(zlib_output.lock().unwrap().bytes, plaintext);

        assert_eq!(plaintext.len(), 29);
        // This valid raw stored block starts with a pair that also passes the
        // two-byte zlib header checksum, so the tentative zlib decoder must
        // reject it and replay the complete first chunk through raw deflate.
        let mut raw_wire = vec![0x08, 0x1d, 0x00, 0xe2, 0xff];
        raw_wire.extend_from_slice(plaintext);
        raw_wire.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
        let raw_output = Arc::new(Mutex::new(DecoderOutput::default()));
        let mut raw = StreamingDeflateDecoder::new(Box::new(OutputWriter(raw_output.clone())));
        raw.write_all(&raw_wire).unwrap();
        raw.flush().unwrap();
        assert_eq!(raw_output.lock().unwrap().bytes, plaintext);
    }

    #[test]
    fn streaming_deflate_probe_retains_at_most_the_configured_prefix() {
        let output = Arc::new(Mutex::new(DecoderOutput::default()));
        let mut decoder = StreamingDeflateDecoder::new(Box::new(OutputWriter(output)));
        let mut empty_blocks = vec![0x78, 0x9c];
        empty_blocks.extend(
            std::iter::repeat_n(
                [0x00, 0x00, 0x00, 0xff, 0xff],
                MAX_DEFLATE_PROBE_INPUT_BYTES / 5 + 2,
            )
            .flatten(),
        );

        decoder.write_all(&empty_blocks).unwrap();
        assert!(matches!(decoder.state, DeflateDecoderState::Zlib(_)));

        let probe_output = Arc::new(Mutex::new(DeflateProbeOutput::default()));
        let mut probe = DeflateProbeWriter(probe_output.clone());
        assert!(probe
            .write_all(&vec![0; MAX_DEFLATE_PROBE_OUTPUT_BYTES + 1])
            .is_err());
        let probe_output = probe_output.lock().unwrap();
        assert!(probe_output.limit_exceeded);
        assert!(probe_output.bytes.is_empty());
        probe.finish_decode().unwrap();
    }

    #[test]
    fn streaming_deflate_finish_handles_empty_and_truncated_streams() {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&[]).unwrap();
        let empty_zlib = encoder.finish().unwrap();
        let output = Arc::new(Mutex::new(DecoderOutput::default()));
        let mut complete = StreamingDeflateDecoder::new(Box::new(OutputWriter(output.clone())));
        complete.write_all(&empty_zlib).unwrap();
        complete.finish_decode().unwrap();
        assert!(output.lock().unwrap().bytes.is_empty());

        let mut truncated = IncrementalContentDecoder::new("deflate", 1024).unwrap();
        assert!(truncated.push(&empty_zlib[..1]).unwrap().is_empty());
        assert!(truncated.finish().is_err());
    }

    #[test]
    fn incremental_decoder_finish_rejects_truncated_standard_streams() {
        let plaintext = b"data: emitted before corrupt trailer\n\n";

        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(plaintext).unwrap();
        let mut gzip_wire = gzip.finish().unwrap();
        gzip_wire.truncate(gzip_wire.len() - 4);

        let mut zstd_wire = zstd::stream::encode_all(plaintext.as_slice(), 1).unwrap();
        zstd_wire.truncate(zstd_wire.len() - 1);

        let mut brotli_wire = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut brotli_wire, 4096, 5, 22);
            encoder.write_all(plaintext).unwrap();
        }
        brotli_wire.truncate(brotli_wire.len() - 1);

        for (encoding, wire) in [
            ("gzip", gzip_wire),
            ("zstd", zstd_wire),
            ("br", brotli_wire),
        ] {
            let mut decoder = IncrementalContentDecoder::new(encoding, 1024).unwrap();
            let _ = decoder.push(&wire);
            assert!(decoder.finish().is_err(), "{encoding}");
        }
    }

    #[tokio::test]
    async fn truncated_encoded_sse_emits_error_without_synthetic_finish() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("truncated-sse.gz");
        let plaintext = b"data: delivered event\n\n";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(plaintext).unwrap();
        let mut wire = encoder.finish().unwrap();
        wire.truncate(wire.len() - 4);
        std::fs::write(&path, &wire).unwrap();
        let body_ref = BodyRef::FileRange {
            path: path.to_string_lossy().to_string(),
            offset: 0,
            size: wire.len(),
        }
        .with_content_encoding(Some("gzip"))
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        stream_content_encoded_sse_events(
            Arc::new(AdminState::new(0)),
            "truncated-sse",
            body_ref,
            SseStreamFrom::Begin,
            1,
            1024,
            tx,
        )
        .await
        .unwrap();

        let mut output = String::new();
        while let Some(bytes) = rx.recv().await {
            output.push_str(std::str::from_utf8(&bytes).unwrap());
        }
        assert!(output.contains("delivered event"));
        assert!(output.contains("\"event\":\"error\""));
        assert!(!output.contains("\"event\":\"finish\""));
    }

    #[test]
    fn incremental_decoder_rejects_unknown_coding_and_enforces_budget() {
        assert!(IncrementalContentDecoder::new("x-company", 1024).is_err());

        for encoding in ["gzip", "x-gzip", "zstd", "deflate, gzip"] {
            let mut decoder = IncrementalContentDecoder::new(encoding, 1024).unwrap();
            assert!(decoder.push(b"n").unwrap().is_empty());
            assert!(decoder.push(b"ot a valid frame").is_err(), "{encoding}");
        }

        let plaintext = b"data: exceeds tiny budget\n\n";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(plaintext).unwrap();
        let wire = encoder.finish().unwrap();
        let mut decoder = IncrementalContentDecoder::new("gzip", 4).unwrap();

        assert!(decoder.push(&wire).is_err());
        assert!(decoder.exceeded_limit());
    }

    #[tokio::test]
    async fn decoded_event_delivery_reports_closed_single_and_batch_channels() {
        for batch_size in [1, 2] {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            drop(rx);
            let mut parser = SseIncrementalParser::new();
            let mut seq = 0;
            let mut batch = Vec::new();
            let mut last_event_was_finish = false;

            assert!(emit_decoded_events(
                b"data: cannot deliver\n\ndata: second event\n\n",
                &mut parser,
                &mut seq,
                &mut batch,
                batch_size,
                &mut last_event_was_finish,
                &tx,
            )
            .await
            .is_err());
        }
    }

    #[tokio::test]
    async fn decode_error_is_flushed_as_a_batch() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let mut seq = 0;
        let mut batch = Vec::new();

        emit_decode_error(&mut seq, &mut batch, 2, &tx).await;

        let payload = rx.recv().await.unwrap();
        assert!(std::str::from_utf8(&payload)
            .unwrap()
            .contains("failed to decode content-encoded SSE body"));
    }
}
