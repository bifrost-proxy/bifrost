use std::cell::Cell;
use std::io::{self, Write};
use std::panic::{self, PanicHookInfo};
use std::sync::atomic::{AtomicBool, Ordering};

static PANIC_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static PANIC_HOOK_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

fn write_panic_diagnostic(
    writer: &mut impl Write,
    thread_name: &str,
    location: &str,
    message: &str,
    backtrace: Option<&std::backtrace::Backtrace>,
) -> io::Result<()> {
    writeln!(
        writer,
        "\n[PANIC] Thread '{thread_name}' panicked at {location}:\n  {message}"
    )?;
    if let Some(backtrace) = backtrace {
        writeln!(writer, "\nBacktrace:\n{backtrace}")?;
    }
    Ok(())
}

pub fn install_panic_hook() {
    if PANIC_HOOK_INSTALLED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let _ = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info: &PanicHookInfo| {
        let already_active = PANIC_HOOK_ACTIVE.with(|active| active.replace(true));
        if already_active {
            return;
        }

        let location = panic_info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic payload".to_string()
        };

        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("unnamed");

        tracing::error!(
            target: "bifrost::panic",
            thread = %thread_name,
            location = %location,
            message = %message,
            "PANIC occurred in thread"
        );

        let backtrace = std::backtrace::Backtrace::capture();
        if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
            tracing::error!(
                target: "bifrost::panic",
                backtrace = %backtrace,
                "Panic backtrace"
            );
        }
        let captured_backtrace =
            (backtrace.status() == std::backtrace::BacktraceStatus::Captured).then_some(&backtrace);
        let mut stderr = std::io::stderr().lock();
        let _ = write_panic_diagnostic(
            &mut stderr,
            thread_name,
            &location,
            &message,
            captured_backtrace,
        );
        PANIC_HOOK_ACTIVE.with(|active| active.set(false));
    }));

    tracing::debug!("Panic hook installed successfully");
}

pub async fn spawn_with_panic_guard<F, T>(
    name: &'static str,
    future: F,
) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(async move {
        let result = tokio::spawn(future).await;

        if let Err(e) = result {
            if e.is_panic() {
                tracing::error!(
                    target: "bifrost::panic",
                    task = %name,
                    error = %e,
                    "Task panicked and was caught by panic guard"
                );
            } else if e.is_cancelled() {
                tracing::debug!(
                    target: "bifrost::task",
                    task = %name,
                    "Task was cancelled"
                );
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_panic_hook_idempotent() {
        install_panic_hook();
        install_panic_hook();
        assert!(PANIC_HOOK_INSTALLED.load(Ordering::SeqCst));
    }

    #[test]
    fn panic_hook_closure_runs_on_str_and_string_payloads() {
        // Ensure the hook is installed so our custom closure body executes.
        install_panic_hook();

        // &str payload branch
        let caught = std::panic::catch_unwind(|| {
            panic!("string-slice panic payload");
        });
        assert!(caught.is_err());

        // String payload branch
        let caught = std::panic::catch_unwind(|| {
            panic!("{}", String::from("owned-string panic payload"));
        });
        assert!(caught.is_err());
    }

    #[test]
    fn panic_hook_reentrant_path_returns_without_recursing() {
        install_panic_hook();
        PANIC_HOOK_ACTIVE.with(|active| active.set(true));
        let caught = std::panic::catch_unwind(|| panic!("nested panic"));
        PANIC_HOOK_ACTIVE.with(|active| active.set(false));
        assert!(caught.is_err());
    }

    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed pipe"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct BacktraceBrokenPipeWriter;

    impl Write for BacktraceBrokenPipeWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf
                .windows("Backtrace".len())
                .any(|part| part == b"Backtrace")
            {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed pipe"))
            } else {
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn panic_diagnostic_propagates_broken_pipe_without_panicking() {
        let result = write_panic_diagnostic(
            &mut BrokenPipeWriter,
            "worker",
            "source.rs:1:1",
            "boom",
            None,
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn panic_diagnostic_includes_an_available_backtrace() {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let mut output = Vec::new();
        write_panic_diagnostic(
            &mut output,
            "worker",
            "source.rs:1:1",
            "boom",
            Some(&backtrace),
        )
        .unwrap();
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Backtrace:"));
    }

    #[test]
    fn panic_diagnostic_propagates_broken_pipe_while_writing_backtrace() {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let result = write_panic_diagnostic(
            &mut BacktraceBrokenPipeWriter,
            "worker",
            "source.rs:1:1",
            "boom",
            Some(&backtrace),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn spawn_with_panic_guard_handles_normal_completion() {
        let handle = spawn_with_panic_guard("normal-task", async { 42 }).await;
        // The guard task should complete without error.
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn spawn_with_panic_guard_catches_panicking_task() {
        install_panic_hook();
        let handle =
            spawn_with_panic_guard("panicky-task", async { panic!("boom inside task") }).await;
        // The outer guard task swallows the inner panic and completes cleanly.
        handle.await.unwrap();
    }
}
