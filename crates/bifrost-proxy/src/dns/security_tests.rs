use super::*;
use hickory_resolver::config::{LookupIpStrategy, ResolveHosts};
use hickory_resolver::net::NetError;
use hickory_resolver::proto::op::Message;
use hickory_resolver::proto::rr::{rdata::A, RData, Record, RecordType};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::task::JoinHandle;

const TEST_HOST: &str = "hickory-security.test.";
const ANSWER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 42);

struct DnsFixture {
    addr: SocketAddr,
    udp_queries: Arc<AtomicUsize>,
    tcp_queries: Arc<AtomicUsize>,
    tasks: Vec<JoinHandle<()>>,
}

impl DnsFixture {
    async fn start(truncate_udp: bool, truncate_tcp: bool) -> Self {
        let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let udp = UdpSocket::bind(addr).await.unwrap();
        let udp_queries = Arc::new(AtomicUsize::new(0));
        let tcp_queries = Arc::new(AtomicUsize::new(0));

        let count = udp_queries.clone();
        let udp_task = tokio::spawn(async move {
            let mut buffer = [0; 4096];
            loop {
                let (len, peer) = udp.recv_from(&mut buffer).await.unwrap();
                count.fetch_add(1, Ordering::SeqCst);
                let response = response(&buffer[..len], truncate_udp);
                udp.send_to(&response, peer).await.unwrap();
            }
        });

        let count = tcp_queries.clone();
        let tcp_task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = tcp.accept().await.unwrap();
                // Keep the connection open and answer every query. Closing it after
                // one TC response would hide the old resolver's unbounded retry loop.
                while let Ok(len) = stream.read_u16().await {
                    let mut buffer = vec![0; usize::from(len)];
                    if stream.read_exact(&mut buffer).await.is_err() {
                        break;
                    }
                    count.fetch_add(1, Ordering::SeqCst);
                    let response = response(&buffer, truncate_tcp);
                    if stream.write_u16(response.len() as u16).await.is_err()
                        || stream.write_all(&response).await.is_err()
                    {
                        break;
                    }
                }
            }
        });

        Self {
            addr,
            udp_queries,
            tcp_queries,
            tasks: vec![udp_task, tcp_task],
        }
    }

    fn resolver(&self, tcp_only: bool) -> TokioResolver {
        // Bifrost's custom-server factory is UDP-only. Explicitly enable TCP
        // here to test the patched dependency through our production builder.
        let mut connections = vec![ConnectionConfig::tcp()];
        if !tcp_only {
            connections.insert(0, ConnectionConfig::udp());
        }
        for connection in &mut connections {
            connection.port = self.addr.port();
        }
        let config = ResolverConfig::from_parts(
            None,
            vec![],
            vec![NameServerConfig::new(self.addr.ip(), true, connections)],
        );
        let mut opts = ResolverOpts::default();
        opts.ip_strategy = LookupIpStrategy::Ipv4Only;
        opts.use_hosts_file = ResolveHosts::Never;
        opts.attempts = 0;
        opts.num_concurrent_reqs = 1;
        opts.timeout = Duration::from_secs(5);
        DnsResolver::build_tokio_resolver(config, opts).unwrap()
    }

    fn counts(&self) -> (usize, usize) {
        (
            self.udp_queries.load(Ordering::SeqCst),
            self.tcp_queries.load(Ordering::SeqCst),
        )
    }
}

impl Drop for DnsFixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn response(request: &[u8], truncated: bool) -> Vec<u8> {
    let request = Message::from_vec(request).unwrap();
    let mut response = Message::response(request.metadata.id, request.metadata.op_code);
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.metadata.recursion_available = true;
    response.metadata.truncation = truncated;
    response.queries = request.queries;
    response.edns = request.edns;
    if !truncated && response.queries[0].query_type() == RecordType::A {
        response.add_answer(Record::from_rdata(
            response.queries[0].name().clone(),
            60,
            RData::A(A(ANSWER)),
        ));
    }
    response.to_vec().unwrap()
}

#[tokio::test]
async fn truncated_udp_and_tcp_responses_have_bounded_retries() {
    let server = DnsFixture::start(true, true).await;
    let resolver = server.resolver(false);
    let result = tokio::time::timeout(Duration::from_secs(2), resolver.lookup_ip(TEST_HOST))
        .await
        .expect("TC responses must fail before the resolver's five-second timeout");

    assert!(matches!(result, Err(NetError::Truncated)), "{result:?}");
    assert_eq!(
        server.counts(),
        (1, 1),
        "UDP then TCP, with no TC retry loop"
    );
}

#[tokio::test]
async fn truncated_tcp_only_responses_have_bounded_retries() {
    let server = DnsFixture::start(true, true).await;
    let resolver = server.resolver(true);
    let result = tokio::time::timeout(Duration::from_secs(2), resolver.lookup_ip(TEST_HOST))
        .await
        .expect("TCP-only TC responses must not spin until the resolver timeout");

    assert!(matches!(result, Err(NetError::Truncated)), "{result:?}");
    assert_eq!(
        server.counts(),
        (0, 2),
        "TCP-only retry must also be bounded"
    );
}

#[tokio::test]
async fn truncated_udp_response_still_falls_back_to_successful_tcp() {
    let server = DnsFixture::start(true, false).await;
    let resolver = server.resolver(false);
    let result = tokio::time::timeout(Duration::from_secs(2), resolver.lookup_ip(TEST_HOST))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.iter().collect::<Vec<_>>(), vec![IpAddr::V4(ANSWER)]);
    assert_eq!(server.counts(), (1, 1));
}

#[tokio::test]
async fn custom_dns_still_resolves_and_caches_successful_udp_answers() {
    let server = DnsFixture::start(false, false).await;
    let resolver = DnsResolver::new(false).with_timeout(Duration::from_secs(2));
    let servers = vec![server.addr.to_string()];

    assert_eq!(
        resolver.resolve(TEST_HOST, &servers).await.unwrap(),
        Some(IpAddr::V4(ANSWER))
    );
    let counts = server.counts();
    assert!(counts.0 > 0);
    assert_eq!(counts.1, 0, "custom DNS configuration remains UDP-only");
    assert_eq!(
        resolver.resolve(TEST_HOST, &servers).await.unwrap(),
        Some(IpAddr::V4(ANSWER))
    );
    assert_eq!(
        server.counts(),
        counts,
        "cached lookup must not query again"
    );
    assert_eq!(resolver.cache_stats().await.total_entries, 1);
}
