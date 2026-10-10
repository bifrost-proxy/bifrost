const http = require("http");
const WebSocket = require("ws");
const { WebSocketServer } = require("ws");
const { HttpProxyAgent } = require("http-proxy-agent");

const proxyUrl = process.env.PROXY_URL || `http://127.0.0.1:${process.env.BIFROST_UI_TEST_PORT || "9910"}`;
const proxyHost = new URL(proxyUrl);

const httpServer = http.createServer((req, res) => {
  const url = req.url || "/";
  if (url.startsWith("/sse")) {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write("id: 1\ndata: alpha\n\n");
    res.write("id: 2\ndata: beta\n\n");
    res.end();
    return;
  }
  res.writeHead(200, { "Content-Type": "application/json" });
  res.end(JSON.stringify({ path: url, ts: Date.now() }));
});

const wss = new WebSocketServer({ port: 0, host: "127.0.0.1" });
wss.on("connection", (socket) => {
  socket.on("message", (data) => {
    socket.send(data);
  });
});

const run = async () => {
  await new Promise((resolve) => httpServer.listen(0, "127.0.0.1", resolve));
  const httpPort = httpServer.address().port;
  await new Promise((resolve) => wss.on("listening", resolve));
  const wsPort = wss.address().port;

  const agent = new HttpProxyAgent(proxyUrl);
  let seq = 0;

  const sendHttp = () => {
    const url = `http://127.0.0.1:${httpPort}/ping-${seq}`;
    const req = http.request(
      {
        host: proxyHost.hostname,
        port: proxyHost.port || 80,
        method: "GET",
        path: url,
        headers: {
          Host: `127.0.0.1:${httpPort}`,
        },
        agent,
      },
      (res) => {
        res.on("data", () => {});
        res.on("end", () => {});
      },
    );
    req.on("error", () => {});
    req.end();
    seq += 1;
  };

  const sendSse = () => {
    const url = `http://127.0.0.1:${httpPort}/sse?seq=${seq}`;
    const req = http.request(
      {
        host: proxyHost.hostname,
        port: proxyHost.port || 80,
        method: "GET",
        path: url,
        headers: {
          Host: `127.0.0.1:${httpPort}`,
        },
        agent,
      },
      (res) => {
        res.on("data", () => {});
        res.on("end", () => {});
      },
    );
    req.on("error", () => {});
    req.end();
  };

  const sendWs = () => {
    const ws = new WebSocket(`ws://127.0.0.1:${wsPort}/ws-${seq}`, { agent });
    ws.on("open", () => {
      ws.send("hello");
      ws.send(Buffer.from([1, 2, 3, 4, 5, 6]));
      ws.close();
    });
    ws.on("error", () => {});
  };

  const httpTimer = setInterval(sendHttp, 2000);
  const sseTimer = setInterval(sendSse, 7000);
  const wsTimer = setInterval(sendWs, 7000);

  const shutdown = () => {
    clearInterval(httpTimer);
    clearInterval(sseTimer);
    clearInterval(wsTimer);
    wss.close();
    httpServer.close(() => process.exit(0));
  };

  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
};

run().catch(() => {
  process.exit(1);
});
