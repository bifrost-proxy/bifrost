use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::process::Stdio;

pub(crate) struct StreamedOutputCapture {
    stdout: tempfile::NamedTempFile,
    stderr: tempfile::NamedTempFile,
    stdout_forwarded: u64,
    stderr_forwarded: u64,
}

impl StreamedOutputCapture {
    pub(crate) fn new() -> io::Result<Self> {
        Ok(Self {
            stdout: tempfile::NamedTempFile::new()?,
            stderr: tempfile::NamedTempFile::new()?,
            stdout_forwarded: 0,
            stderr_forwarded: 0,
        })
    }

    pub(crate) fn stdout_stdio(&self) -> io::Result<Stdio> {
        self.stdout.reopen().map(Stdio::from)
    }

    pub(crate) fn stderr_stdio(&self) -> io::Result<Stdio> {
        self.stderr.reopen().map(Stdio::from)
    }

    /// Forward newly captured bytes and report whether the child produced any
    /// fresh output since the previous call.
    pub(crate) fn forward_available(&mut self) -> bool {
        let before = (self.stdout_forwarded, self.stderr_forwarded);
        let mut stdout = io::stdout().lock();
        let mut stderr = io::stderr().lock();
        let _ = self.forward_available_to(&mut stdout, &mut stderr);
        before != (self.stdout_forwarded, self.stderr_forwarded)
    }

    fn forward_available_to(
        &mut self,
        stdout_target: &mut impl Write,
        stderr_target: &mut impl Write,
    ) -> io::Result<()> {
        forward_new_bytes(
            self.stdout.as_file_mut(),
            &mut self.stdout_forwarded,
            stdout_target,
        )?;
        forward_new_bytes(
            self.stderr.as_file_mut(),
            &mut self.stderr_forwarded,
            stderr_target,
        )?;
        stdout_target.flush()?;
        stderr_target.flush()
    }

    pub(crate) fn read_all(&mut self) -> (String, String) {
        (
            read_complete_output(self.stdout.as_file_mut()),
            read_complete_output(self.stderr.as_file_mut()),
        )
    }
}

fn forward_new_bytes(file: &mut File, offset: &mut u64, target: &mut impl Write) -> io::Result<()> {
    file.seek(SeekFrom::Start(*offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    *offset = (*offset).saturating_add(bytes.len() as u64);
    target.write_all(&bytes)
}

fn read_complete_output(file: &mut File) -> String {
    if file.seek(SeekFrom::Start(0)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("intentional writer failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn forwards_only_new_bytes_and_retains_complete_output() {
        let mut capture = StreamedOutputCapture::new().expect("capture");
        let mut child_stdout = capture.stdout.reopen().expect("stdout writer");
        let mut child_stderr = capture.stderr.reopen().expect("stderr writer");
        child_stdout.write_all(b"download 10%\r").unwrap();
        child_stderr.write_all(b"installer note\n").unwrap();
        child_stdout.flush().unwrap();
        child_stderr.flush().unwrap();

        let mut visible_stdout = Vec::new();
        let mut visible_stderr = Vec::new();
        capture
            .forward_available_to(&mut visible_stdout, &mut visible_stderr)
            .unwrap();
        assert_eq!(visible_stdout, b"download 10%\r");
        assert_eq!(visible_stderr, b"installer note\n");

        child_stdout.write_all(b"download 100%\n").unwrap();
        child_stdout.flush().unwrap();
        capture
            .forward_available_to(&mut visible_stdout, &mut visible_stderr)
            .unwrap();
        assert_eq!(
            visible_stdout, b"download 10%\rdownload 100%\n",
            "the second tail must not duplicate already-forwarded bytes"
        );

        let (captured_stdout, captured_stderr) = capture.read_all();
        assert_eq!(captured_stdout, "download 10%\rdownload 100%\n");
        assert_eq!(captured_stderr, "installer note\n");
    }

    #[test]
    fn forwarding_reports_only_fresh_child_output_as_activity() {
        let mut capture = StreamedOutputCapture::new().expect("capture");
        assert!(!capture.forward_available());

        let mut child_stdout = capture.stdout.reopen().expect("stdout writer");
        child_stdout.write_all(b"download 10%\r").unwrap();
        child_stdout.flush().unwrap();

        assert!(capture.forward_available());
        assert!(
            !capture.forward_available(),
            "already-forwarded bytes cannot extend a stall deadline again"
        );
    }

    #[test]
    fn forwarding_propagates_stdout_and_stderr_write_failures() {
        let mut stdout_capture = StreamedOutputCapture::new().expect("stdout capture");
        stdout_capture
            .stdout
            .as_file_mut()
            .write_all(b"stdout")
            .unwrap();
        let stdout_error = stdout_capture
            .forward_available_to(&mut FailingWriter, &mut Vec::new())
            .expect_err("stdout target failure must propagate");
        assert_eq!(stdout_error.kind(), io::ErrorKind::Other);

        let mut stderr_capture = StreamedOutputCapture::new().expect("stderr capture");
        stderr_capture
            .stderr
            .as_file_mut()
            .write_all(b"stderr")
            .unwrap();
        let stderr_error = stderr_capture
            .forward_available_to(&mut Vec::new(), &mut FailingWriter)
            .expect_err("stderr target failure must propagate");
        assert_eq!(stderr_error.kind(), io::ErrorKind::Other);
    }
}
