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
            test_type: TestCaseType::Standalone {
                test_fn: Arc::new(move || Box::pin(test_fn())),
            },
        }
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
}

impl TestRunner {
    pub fn new(base_port: u16, reporter: Reporter) -> Self {
        Self {
            tests: Vec::new(),
            base_port,
            concurrency: 1,
            reporter,
        }
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
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

        for &idx in &failed_indices {
            let test = &self.tests[idx];
            let port = self.base_port + (idx as u16);
            tracing::info!("  Retrying: {} (port {})", test.name, port);
            let result = run_single_test(test, port).await;
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
    }

    async fn run_all_serial(&mut self) -> Vec<TestResult> {
        let mut results = Vec::new();
        let total = self.tests.len();

        for (i, test) in self.tests.iter().enumerate() {
            let port = self.base_port + (i as u16);
            let result = run_single_test(test, port).await;
            self.reporter.report_test(&result, i + 1, total);
            results.push(result);
        }

        self.reporter.summary(&results);
        results
    }

    async fn run_all_parallel(&mut self, total: usize) -> Vec<TestResult> {
        let semaphore = Arc::new(Semaphore::new(self.concurrency));
        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let mut handles = Vec::with_capacity(total);

        for (i, test) in self.tests.iter().enumerate() {
            let port = self.base_port + (i as u16);
            let sem = semaphore.clone();
            let completed = completed.clone();
            let test = test.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let result = run_single_test(&test, port).await;

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

                result
            });

            handles.push(handle);
        }

        let mut results = Vec::with_capacity(total);
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(TestResult {
                    name: "unknown".to_string(),
                    category: "unknown".to_string(),
                    status: TestStatus::Failed,
                    duration: Duration::ZERO,
                    error: Some(format!("Task panicked: {}", e)),
                }),
            }
        }

        for (i, result) in results.iter().enumerate() {
            self.reporter.report_test(result, i + 1, total);
        }
        self.reporter.summary(&results);
        results
    }
}

async fn run_single_test(test: &TestCase, port: u16) -> TestResult {
    let start = Instant::now();

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
}

impl Default for TestRunner {
    fn default() -> Self {
        Self::new(18800, Reporter::new(false))
    }
}
