use crate::client::ProxyClient;
use crate::mock::HttpbinMockServer;
use crate::proxy::ProxyInstance;
use crate::reporter::Reporter;
use crate::tests;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

pub type TestFn = Arc<
    dyn Fn(ProxyClient) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync,
>;

pub type StandaloneTestFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub category: String,
    pub status: TestStatus,
    pub duration: Duration,
    pub error: Option<String>,
}

impl TestResult {
    pub fn passed() -> Self {
        Self {
            name: String::new(),
            category: String::new(),
            status: TestStatus::Passed,
            duration: Duration::ZERO,
            error: None,
        }
    }

    pub fn failed(error: &str) -> Self {
        Self {
            name: String::new(),
            category: String::new(),
            status: TestStatus::Failed,
            duration: Duration::ZERO,
            error: Some(error.to_string()),
        }
    }

    pub fn skipped(reason: &str) -> Self {
        Self {
            name: String::new(),
            category: String::new(),
            status: TestStatus::Skipped,
            duration: Duration::ZERO,
            error: Some(reason.to_string()),
        }
    }
}

impl From<Result<(), String>> for TestResult {
    fn from(result: Result<(), String>) -> Self {
        match result {
            Ok(()) => TestResult::passed(),
            Err(e) if e.starts_with("SKIPPED:") => TestResult::skipped(&e),
            Err(e) => TestResult::failed(&e),
        }
    }
}

#[derive(Clone)]
pub enum TestCaseType {
    Standard { rules: Vec<String>, test_fn: TestFn },
    Standalone { test_fn: StandaloneTestFn },
}

#[derive(Clone)]
pub struct TestCase {
    pub name: String,
    pub description: String,
    pub category: String,
    pub parallel_safe: bool,
    test_type: TestCaseType,
}

impl TestCase {
    pub fn new<F, Fut>(name: &str, category: &str, rules: Vec<&str>, test_fn: F) -> Self
    where
        F: Fn(ProxyClient) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        Self {
            name: name.to_string(),
            description: String::new(),
            category: category.to_string(),
            parallel_safe: true,
            test_type: TestCaseType::Standard {
                rules: rules.iter().map(|s| s.to_string()).collect(),
                test_fn: Arc::new(move |client| Box::pin(test_fn(client))),
            },
        }
    }

    pub fn standalone<F, Fut>(name: &str, description: &str, category: &str, test_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            category: category.to_string(),
            parallel_safe: true,
            test_type: TestCaseType::Standalone {
                test_fn: Arc::new(move || Box::pin(test_fn())),
            },
        }
    }

    pub fn serial(mut self) -> Self {
        self.parallel_safe = false;
        self
    }

    pub fn rules(&self) -> Option<&[String]> {
        match &self.test_type {
            TestCaseType::Standard { rules, .. } => Some(rules),
            TestCaseType::Standalone { .. } => None,
        }
    }
}

pub struct TestRunner {
    tests: Vec<TestCase>,
    base_port: u16,
    concurrency: usize,
    reporter: Reporter,
    global_timeout: Option<Duration>,
    test_timeout: Duration,
}

impl TestRunner {
    pub fn new(base_port: u16, reporter: Reporter) -> Self {
        let base_port = if base_port < 1024 {
            tracing::warn!(
                "base_port {} is in privileged range, overriding to 18080",
                base_port
            );
            18080
        } else {
            base_port
        };
        Self {
            tests: Vec::new(),
            base_port,
            concurrency: 1,
            reporter,
            global_timeout: None,
            test_timeout: Duration::from_secs(120),
        }
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    pub fn with_global_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.global_timeout = timeout;
        self
    }

    pub fn with_test_timeout(mut self, timeout: Duration) -> Self {
        self.test_timeout = timeout;
        self
    }

    pub fn load_all_tests(&mut self) {
        self.tests = tests::all_tests();
    }

    pub fn add_test(&mut self, test: TestCase) {
        self.tests.push(test);
    }

    pub fn add_tests(&mut self, tests: Vec<TestCase>) {
        self.tests.extend(tests);
    }

    pub fn filter_by_category(&mut self, category: &str) {
        self.tests.retain(|t| t.category == category);
    }

    pub fn filter_by_name(&mut self, pattern: &str) {
        self.tests.retain(|t| t.name.contains(pattern));
    }

    pub fn list_tests(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for test in &self.tests {
            map.entry(test.category.clone())
                .or_default()
                .push(test.name.clone());
        }
        map
    }

    pub fn reporter(&self) -> &Reporter {
        &self.reporter
    }

    pub async fn run_all(&mut self) -> Vec<TestResult> {
        let total = self.tests.len();
        self.reporter.start(total);

        let global_timeout = self.global_timeout;

        let run_tests = async {
            let mut results = if self.concurrency <= 1 {
                self.run_all_serial().await
            } else {
                self.run_all_parallel(total).await
            };

            let retry_enabled = std::env::var("BIFROST_E2E_RETRY_FAILED_ONCE")
                .ok()
                .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

            if !retry_enabled {
                return results;
            }

            let failed_indices: Vec<usize> = results
                .iter()
                .enumerate()
                .filter(|(_, r)| r.status == TestStatus::Failed)
                .map(|(i, _)| i)
                .collect();

            if failed_indices.is_empty() {
                return results;
            }

            tracing::info!("Retrying {} failed test(s) once...", failed_indices.len());

            let test_timeout = self.test_timeout;
            let total_tests = self.tests.len() as u16;
            for &idx in &failed_indices {
                let test = self.tests[idx].clone();
                let retry_port = self.base_port + total_tests + (idx as u16);
                wait_for_port_available(retry_port).await;
                tracing::info!("  Retrying: {} (port {})", test.name, retry_port);
                let result = run_retry_test(test, retry_port, test_timeout).await;
                tracing::info!(
                    "  Retry result: {} {} ({}ms)",
                    match result.status {
                        TestStatus::Passed => "✓",
                        TestStatus::Failed => "✗",
                        TestStatus::Skipped => "○",
                    },
                    result.name,
                    result.duration.as_millis()
                );
                if result.status == TestStatus::Failed {
                    if let Some(ref error) = result.error {
                        tracing::error!("  RETRY FAIL: {} - {}", result.name, error);
                    }
                }
                results[idx] = result;
            }

            results
        };

        if let Some(timeout) = global_timeout {
            match tokio::time::timeout(timeout, run_tests).await {
                Ok(results) => results,
                Err(_) => {
                    tracing::error!(
                        "Global timeout reached after {}s, aborting remaining tests",
                        timeout.as_secs()
                    );
                    Vec::new()
                }
            }
        } else {
            run_tests.await
        }
    }

    async fn run_all_serial(&mut self) -> Vec<TestResult> {
        let mut results = Vec::new();
        let total = self.tests.len();
        let test_timeout = self.test_timeout;

        for (i, test) in self.tests.iter().enumerate() {
            let port = self.base_port + (i as u16);
            let result = run_single_test(test, port, test_timeout).await;
            self.reporter.report_test(&result, i + 1, total);
            results.push(result);
        }

        self.reporter.summary(&results);
        results
    }

    async fn run_all_parallel(&mut self, total: usize) -> Vec<TestResult> {
        let semaphore = Arc::new(Semaphore::new(self.concurrency));
        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let test_timeout = self.test_timeout;
        let mut indexed_results: Vec<Option<TestResult>> = vec![None; total];

        let mut handles = Vec::with_capacity(total);

        for (i, test) in self.tests.iter().enumerate() {
            if !test.parallel_safe {
                continue;
            }
            let port = self.base_port + (i as u16);
            let sem = semaphore.clone();
            let completed = completed.clone();
            let test = test.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let result = run_single_test(&test, port, test_timeout).await;

                let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                tracing::info!(
                    "[{}/{}] {} {} ({}ms)",
                    done,
                    total,
                    match result.status {
                        TestStatus::Passed => "✓",
                        TestStatus::Failed => "✗",
                        TestStatus::Skipped => "○",
                    },
                    result.name,
                    result.duration.as_millis()
                );
                if result.status == TestStatus::Failed {
                    if let Some(ref error) = result.error {
                        tracing::error!("  FAIL: {} - {}", result.name, error);
                    }
                }

                (i, result)
            });

            handles.push(handle);
        }

        for handle in handles {
            match handle.await {
                Ok((idx, result)) => indexed_results[idx] = Some(result),
                Err(e) => {
                    let result = TestResult {
                        name: "unknown".to_string(),
                        category: "unknown".to_string(),
                        status: TestStatus::Failed,
                        duration: Duration::ZERO,
                        error: Some(format!("Task panicked: {}", e)),
                    };
                    indexed_results.push(Some(result));
                }
            }
        }

        for (i, test) in self.tests.iter().enumerate() {
            if test.parallel_safe {
                continue;
            }
            tracing::info!("Running serial-only test: {}", test.name);
            let port = self.base_port + (i as u16);
            wait_for_port_available(port).await;
            let result = run_single_test(test, port, test_timeout).await;
            let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            tracing::info!(
                "[{}/{}] {} {} ({}ms)",
                done,
                total,
                match result.status {
                    TestStatus::Passed => "✓",
                    TestStatus::Failed => "✗",
                    TestStatus::Skipped => "○",
                },
                result.name,
                result.duration.as_millis()
            );
            if result.status == TestStatus::Failed {
                if let Some(ref error) = result.error {
                    tracing::error!("  FAIL: {} - {}", result.name, error);
                }
            }
            indexed_results[i] = Some(result);
        }

        let results: Vec<TestResult> = indexed_results.into_iter().flatten().collect();

        for (i, result) in results.iter().enumerate() {
            self.reporter.report_test(result, i + 1, total);
        }
        self.reporter.summary(&results);
        results
    }
}

async fn run_retry_test(test: TestCase, port: u16, test_timeout: Duration) -> TestResult {
    let name = test.name.clone();
    let category = test.category.clone();

    // The initial parallel run executes tests in spawned runtime tasks, but retrying inline would
    // poll the same test future on the thread calling Runtime::block_on. That thread has a smaller
    // stack on Windows, so a test that ran normally on a worker could overflow only on retry.
    // Keep retry attempts on the same isolated worker-task execution model as the initial run.
    match tokio::spawn(async move { run_single_test(&test, port, test_timeout).await }).await {
        Ok(result) => result,
        Err(error) => TestResult {
            name,
            category,
            status: TestStatus::Failed,
            duration: Duration::ZERO,
            error: Some(format!("retry task failed: {error}")),
        },
    }
}

async fn run_single_test(test: &TestCase, port: u16, test_timeout: Duration) -> TestResult {
    let start = Instant::now();

    let run = async {
        match &test.test_type {
            TestCaseType::Standard { rules, test_fn } => {
                let mut owned_rules = rules.clone();
                let _httpbin = if rules.iter().any(|rule| rule.contains("httpbin.org")) {
                    let mock = HttpbinMockServer::start().await;
                    let mut injected = mock.http_rules();
                    injected.append(&mut owned_rules);
                    owned_rules = injected;
                    Some(mock)
                } else {
                    None
                };

                let rule_refs: Vec<&str> = owned_rules.iter().map(|s| s.as_str()).collect();

                let proxy = match ProxyInstance::start(port, rule_refs).await {
                    Ok(p) => p,
                    Err(e) => {
                        return TestResult {
                            name: test.name.clone(),
                            category: test.category.clone(),
                            status: TestStatus::Failed,
                            duration: start.elapsed(),
                            error: Some(format!("Failed to start proxy: {}", e)),
                        };
                    }
                };

                let client = match ProxyClient::new(&proxy.proxy_url()) {
                    Ok(c) => c,
                    Err(e) => {
                        return TestResult {
                            name: test.name.clone(),
                            category: test.category.clone(),
                            status: TestStatus::Failed,
                            duration: start.elapsed(),
                            error: Some(format!("Failed to create client: {}", e)),
                        };
                    }
                };

                let result = (test_fn)(client).await;
                let duration = start.elapsed();
                let status = match &result {
                    Ok(()) => TestStatus::Passed,
                    Err(error) if error.starts_with("SKIPPED:") => TestStatus::Skipped,
                    Err(_) => TestStatus::Failed,
                };

                TestResult {
                    name: test.name.clone(),
                    category: test.category.clone(),
                    status,
                    duration,
                    error: result.err(),
                }
            }
            TestCaseType::Standalone { test_fn } => {
                let result = (test_fn)().await;
                let duration = start.elapsed();
                let status = match &result {
                    Ok(()) => TestStatus::Passed,
                    Err(error) if error.starts_with("SKIPPED:") => TestStatus::Skipped,
                    Err(_) => TestStatus::Failed,
                };
                TestResult {
                    name: test.name.clone(),
                    category: test.category.clone(),
                    status,
                    duration,
                    error: result.err(),
                }
            }
        }
    };

    match tokio::time::timeout(test_timeout, run).await {
        Ok(result) => result,
        Err(_) => TestResult {
            name: test.name.clone(),
            category: test.category.clone(),
            status: TestStatus::Failed,
            duration: start.elapsed(),
            error: Some(format!("test timed out after {}s", test_timeout.as_secs())),
        },
    }
}

async fn wait_for_port_available(port: u16) {
    use std::net::TcpListener;
    for attempt in 0..30 {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return;
        }
        if attempt == 0 {
            tracing::info!("  Waiting for port {} to become available...", port);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tracing::warn!("  Port {} may still be in use after waiting", port);
}

impl Default for TestRunner {
    fn default() -> Self {
        Self::new(18800, Reporter::new(false))
    }
}

#[cfg(test)]
mod runner_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[tokio::test]
    async fn parallel_runner_keeps_result_order_and_runs_serial_tests_after_parallel_tests() {
        let sequence = Arc::new(AtomicUsize::new(0));
        let serial_seen = Arc::new(AtomicUsize::new(0));

        let mut runner = TestRunner::new(21000, Reporter::new(false)).with_concurrency(2);

        runner.add_test(TestCase::standalone("parallel-one", "", "unit", {
            let sequence = Arc::clone(&sequence);
            let serial_seen = Arc::clone(&serial_seen);
            move || {
                let sequence = Arc::clone(&sequence);
                let serial_seen = Arc::clone(&serial_seen);
                async move {
                    if serial_seen.load(Ordering::SeqCst) != 0 {
                        return Err(
                            "serial test ran before all parallel tests finished".to_string()
                        );
                    }
                    sequence.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        }));

        runner.add_test(
            TestCase::standalone("serial-middle", "", "unit", {
                let sequence = Arc::clone(&sequence);
                let serial_seen = Arc::clone(&serial_seen);
                move || {
                    let sequence = Arc::clone(&sequence);
                    let serial_seen = Arc::clone(&serial_seen);
                    async move {
                        let before = sequence.load(Ordering::SeqCst);
                        if before != 2 {
                            return Err(format!(
                                "serial test started before parallel tests completed: {before}"
                            ));
                        }
                        serial_seen.fetch_add(1, Ordering::SeqCst);
                        sequence.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                }
            })
            .serial(),
        );

        runner.add_test(TestCase::standalone("parallel-two", "", "unit", {
            let sequence = Arc::clone(&sequence);
            let serial_seen = Arc::clone(&serial_seen);
            move || {
                let sequence = Arc::clone(&sequence);
                let serial_seen = Arc::clone(&serial_seen);
                async move {
                    if serial_seen.load(Ordering::SeqCst) != 0 {
                        return Err(
                            "serial test ran before all parallel tests finished".to_string()
                        );
                    }
                    sequence.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        }));

        let results = runner.run_all().await;

        assert_eq!(
            results
                .iter()
                .map(|result| result.name.as_str())
                .collect::<Vec<_>>(),
            vec!["parallel-one", "serial-middle", "parallel-two"]
        );
        assert!(results
            .iter()
            .all(|result| result.status == TestStatus::Passed));
        assert_eq!(sequence.load(Ordering::SeqCst), 3);
        assert_eq!(serial_seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retry_test_runs_on_runtime_worker_instead_of_block_on_thread() {
        let block_on_thread = std::thread::current().id();
        let retry_thread = Arc::new(Mutex::new(None));
        let test = TestCase::standalone("retry-worker", "", "unit", {
            let retry_thread = Arc::clone(&retry_thread);
            move || {
                let retry_thread = Arc::clone(&retry_thread);
                async move {
                    *retry_thread.lock().expect("retry thread lock poisoned") =
                        Some(std::thread::current().id());
                    Ok(())
                }
            }
        });

        let result = run_retry_test(test, 21003, Duration::from_secs(1)).await;

        assert_eq!(result.status, TestStatus::Passed);
        assert_ne!(
            retry_thread
                .lock()
                .expect("retry thread lock poisoned")
                .expect("retry test did not record its thread"),
            block_on_thread
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retry_test_converts_spawned_task_panic_to_failed_result() {
        let test = TestCase::standalone("retry-panic", "", "unit", || async {
            panic!("intentional retry panic");
        });

        let result = run_retry_test(test, 21004, Duration::from_secs(1)).await;

        assert_eq!(result.status, TestStatus::Failed);
        assert_eq!(result.name, "retry-panic");
        assert!(
            result.error.as_deref().is_some_and(
                |error| error.contains("retry task failed") && error.contains("panicked")
            )
        );
    }
}
