use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

fn pick_addr() -> SocketAddr {
    let s = TcpListener::bind("127.0.0.1:0").expect("bind");
    let a = s.local_addr().expect("addr");
    drop(s);
    a
}

#[tokio::test]
async fn prometheus_exporter_serves_metrics() {
    let addr = pick_addr();
    PrometheusBuilder::new()
        .with_http_listener(addr)
        .install()
        .expect("install exporter");

    metrics::counter!("hum_test_counter").increment(7);
    metrics::gauge!("hum_test_gauge").set(42.0);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let url = format!("http://{addr}/metrics");
    let resp = match tokio::time::timeout(Duration::from_secs(2), async {
        let stream = tokio::net::TcpStream::connect(addr).await?;
        let (mut r, mut w) = stream.into_split();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        w.write_all(format!("GET /metrics HTTP/1.0\r\nHost: {addr}\r\n\r\n").as_bytes()).await?;
        let mut body = Vec::new();
        r.read_to_end(&mut body).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&body).into_owned())
    }).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => panic!("connect to {url}: {e}"),
        Err(_) => panic!("timeout fetching {url}"),
    };

    assert!(resp.contains("hum_test_counter"), "missing counter in:\n{resp}");
    assert!(resp.contains("hum_test_gauge"), "missing gauge in:\n{resp}");
    assert!(resp.contains(" 7"), "counter value missing in:\n{resp}");
    assert!(resp.contains(" 42"), "gauge value missing in:\n{resp}");
}
