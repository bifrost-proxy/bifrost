#!/usr/bin/env node

const readline = require("readline");

const TOOL_COUNT = 100;

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function toolName(index) {
  return index === 87 ? "target_tool_087" : `bulk_tool_${String(index).padStart(3, "0")}`;
}

function tools() {
  return Array.from({ length: TOOL_COUNT }, (_, index) => ({
    name: toolName(index),
    description:
      index === 87
        ? "Unique deferred MCP validation tool. Search keyword: needle_087. Returns MCP_TOOL_087_OK."
        : `Bulk deferred MCP validation tool ${index}. Search keyword: bulk_needle_${String(index).padStart(3, "0")}.`,
    inputSchema: {
      type: "object",
      properties: {
        marker: {
          type: "string",
          description: "Marker echoed by the MCP tool.",
        },
      },
    },
  }));
}

async function handle(request) {
  if (request.method === "initialize") {
    send({
      jsonrpc: "2.0",
      id: request.id,
      result: {
        protocolVersion: "2024-11-05",
        capabilities: {
          tools: {},
        },
        serverInfo: {
          name: "many-mcp-tools-test",
          version: "0.0.1",
        },
      },
    });
    return;
  }

  if (request.method === "notifications/initialized") {
    return;
  }

  if (request.method === "tools/list") {
    send({
      jsonrpc: "2.0",
      id: request.id,
      result: {
        tools: tools(),
      },
    });
    return;
  }

  if (request.method === "tools/call") {
    const name = request.params?.name;
    const marker = request.params?.arguments?.marker ?? "";
    const text =
      name === "target_tool_087"
        ? `MCP_TOOL_087_OK marker=${marker}`
        : `MCP_BULK_TOOL_CALLED name=${name} marker=${marker}`;
    send({
      jsonrpc: "2.0",
      id: request.id,
      result: {
        content: [
          {
            type: "text",
            text,
          },
        ],
      },
    });
    return;
  }

  if (request.id !== undefined) {
    send({
      jsonrpc: "2.0",
      id: request.id,
      error: {
        code: -32601,
        message: `method not found: ${request.method}`,
      },
    });
  }
}

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

rl.on("line", (line) => {
  if (!line.trim()) {
    return;
  }
  try {
    const request = JSON.parse(line);
    handle(request).catch((error) => {
      if (request.id !== undefined) {
        send({
          jsonrpc: "2.0",
          id: request.id,
          error: {
            code: -32603,
            message: error.message,
          },
        });
      }
    });
  } catch (error) {
    process.stderr.write(`invalid json: ${error.message}\n`);
  }
});
