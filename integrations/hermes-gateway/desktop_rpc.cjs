'use strict';

const DEFAULT_CONNECT_TIMEOUT_MS = 5000;
const DEFAULT_REQUEST_TIMEOUT_MS = 30000;
const CLOSE_WAIT_TIMEOUT_MS = 1000;

const ALLOWED_METHODS = new Set([
  'projects.list',
  'projects.get',
  'projects.create',
  'projects.for_cwd',
  'projects.project_sessions',
  'session.create',
  'session.resume',
  'session.activate',
  'session.active_list',
  'prompt.submit',
]);

const LOOPBACK_HOSTNAMES = new Set(['127.0.0.1', 'localhost', '[::1]']);

function sanitizeText(value) {
  return String(value ?? '')
    .replace(/token=[^&\s"']+/gi, 'token=[REDACTED]')
    .replace(/window\.__HERMES_SESSION_TOKEN__\s*=\s*("(?:\\.|[^"\\])*")/g, 'window.__HERMES_SESSION_TOKEN__="[REDACTED]"');
}

function parseArgs(argv) {
  let endpoint = '';
  let connectTimeoutMs = DEFAULT_CONNECT_TIMEOUT_MS;
  let requestTimeoutMs = DEFAULT_REQUEST_TIMEOUT_MS;

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--endpoint') {
      endpoint = argv[++i] ?? '';
    } else if (arg.startsWith('--endpoint=')) {
      endpoint = arg.slice('--endpoint='.length);
    } else if (arg === '--connect-timeout-ms') {
      connectTimeoutMs = Number(argv[++i]);
    } else if (arg.startsWith('--connect-timeout-ms=')) {
      connectTimeoutMs = Number(arg.slice('--connect-timeout-ms='.length));
    } else if (arg === '--request-timeout-ms') {
      requestTimeoutMs = Number(argv[++i]);
    } else if (arg.startsWith('--request-timeout-ms=')) {
      requestTimeoutMs = Number(arg.slice('--request-timeout-ms='.length));
    } else {
      throw new Error(`Unsupported argument: ${sanitizeText(arg)}`);
    }
  }

  if (!Number.isFinite(connectTimeoutMs) || connectTimeoutMs <= 0) {
    throw new Error('Invalid --connect-timeout-ms');
  }
  if (!Number.isFinite(requestTimeoutMs) || requestTimeoutMs <= 0) {
    throw new Error('Invalid --request-timeout-ms');
  }

  return {
    endpoint: validateLoopbackEndpoint(endpoint),
    connectTimeoutMs: Math.floor(connectTimeoutMs),
    requestTimeoutMs: Math.floor(requestTimeoutMs),
  };
}

function validateLoopbackEndpoint(rawEndpoint) {
  if (typeof rawEndpoint !== 'string' || !rawEndpoint.trim()) {
    throw new Error('Missing required --endpoint http://127.0.0.1:<port>');
  }
  let parsed;
  try {
    parsed = new URL(rawEndpoint.trim());
  } catch {
    throw new Error('Invalid --endpoint URL');
  }
  if (parsed.protocol !== 'http:') {
    throw new Error('Only http: loopback endpoints are allowed');
  }
  if (!LOOPBACK_HOSTNAMES.has(parsed.hostname.toLowerCase())) {
    throw new Error('Only loopback hostnames (127.0.0.1, localhost, [::1]) are allowed');
  }
  if (!parsed.port) {
    throw new Error('Explicit port is required in --endpoint');
  }
  const portNumber = Number(parsed.port);
  if (!Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535) {
    throw new Error('Invalid port in --endpoint');
  }
  if (parsed.username || parsed.password) {
    throw new Error('Credentials are not allowed in --endpoint');
  }
  if (parsed.pathname !== '/' || parsed.search || parsed.hash) {
    throw new Error('--endpoint must not include path, query, or hash');
  }
  return {
    httpUrl: `http://${parsed.host}/`,
    wsBaseUrl: `ws://${parsed.host}/api/ws`,
  };
}

function idKey(id) {
  return `${typeof id}:${String(id)}`;
}

function writeJsonLine(payload) {
  if (!process.stdout.writable) return;
  process.stdout.write(`${JSON.stringify(payload)}\n`);
}

function writeErrorResponse(id, code, message, data) {
  const error = { code, message: sanitizeText(message) };
  if (data !== undefined) {
    error.data = data;
  }
  writeJsonLine({ id, error });
}

function main() {
  let config;
  try {
    config = parseArgs(process.argv.slice(2));
  } catch (err) {
    process.stderr.write(`${sanitizeText(err.message)}\n`);
    process.exitCode = 1;
    return;
  }

  const EOF_DRAIN_TIMEOUT_MS = 150;
  let socket = null;
  let connectPromise = null;
  let connectAbortController = null;
  let shuttingDown = false;
  let stdinEnded = false;
  let eofDrainTimer = null;
  let stdinBuffer = '';
  const pending = new Map();
  const preConnectQueue = [];

  function checkEofDrainComplete() {
    if (!stdinEnded || shuttingDown) return;
    if (preConnectQueue.length === 0 && pending.size === 0) {
      if (eofDrainTimer) {
        clearTimeout(eofDrainTimer);
        eofDrainTimer = null;
      }
      initiateShutdown(0);
    }
  }

  function clearPending(reasonMessage) {
    for (const item of pending.values()) {
      clearTimeout(item.timer);
      if (reasonMessage) {
        writeErrorResponse(item.id, -32001, reasonMessage);
      }
    }
    pending.clear();
    checkEofDrainComplete();
  }

  function clearPreConnectQueue(reasonMessage) {
    while (preConnectQueue.length > 0) {
      const req = preConnectQueue.shift();
      if (reasonMessage) {
        writeErrorResponse(req.id, -32001, reasonMessage);
      }
    }
    checkEofDrainComplete();
  }

  function closeSocketGracefully() {
    return new Promise((resolve) => {
      if (!socket) {
        resolve();
        return;
      }
      const ws = socket;
      socket = null;
      if (ws.readyState === WebSocket.CLOSED) {
        resolve();
        return;
      }
      const timer = setTimeout(resolve, CLOSE_WAIT_TIMEOUT_MS);
      if (typeof timer.unref === 'function') timer.unref();
      ws.addEventListener(
        'close',
        () => {
          clearTimeout(timer);
          resolve();
        },
        { once: true }
      );
      try {
        if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
          ws.close(1000, 'Client shutdown');
        } else {
          clearTimeout(timer);
          resolve();
        }
      } catch {
        clearTimeout(timer);
        resolve();
      }
    });
  }

  function initiateShutdown(exitCode = 0) {
    if (shuttingDown) return;
    shuttingDown = true;
    if (eofDrainTimer) {
      clearTimeout(eofDrainTimer);
      eofDrainTimer = null;
    }
    if (exitCode !== 0 && !process.exitCode) {
      process.exitCode = exitCode;
    }

    process.stdin.pause();
    process.stdin.removeAllListeners('data');
    process.stdin.removeAllListeners('end');
    process.stdin.removeAllListeners('error');

    if (connectAbortController) {
      try {
        connectAbortController.abort();
      } catch {
        // ignore
      }
      connectAbortController = null;
    }

    clearPreConnectQueue(null);
    clearPending(null);
    void closeSocketGracefully();
  }

  async function establishConnection() {
    connectAbortController = new AbortController();
    const timeoutSignal = AbortSignal.timeout(config.connectTimeoutMs);
    const combinedSignal = AbortSignal.any
      ? AbortSignal.any([connectAbortController.signal, timeoutSignal])
      : timeoutSignal;

    let response;
    try {
      response = await fetch(config.endpoint.httpUrl, { signal: combinedSignal, redirect: 'error' });
    } catch (err) {
      throw new Error(`Hermes HTTP probe failed: ${sanitizeText(err.message)}`);
    }

    if (!response.ok) {
      throw new Error(`Hermes dashboard HTTP status ${response.status}`);
    }

    const html = await response.text();
    const match = /window\.__HERMES_SESSION_TOKEN__\s*=\s*("(?:\\.|[^"\\])*")/.exec(html);
    if (!match) {
      throw new Error('Hermes dashboard did not publish window.__HERMES_SESSION_TOKEN__');
    }

    let token;
    try {
      token = JSON.parse(match[1]);
    } catch {
      throw new Error('Hermes session token parse failed');
    }
    if (typeof token !== 'string' || !token) {
      throw new Error('Hermes session token is empty');
    }

    if (shuttingDown) {
      throw new Error('Shutdown in progress');
    }

    const ws = new WebSocket(`${config.endpoint.wsBaseUrl}?token=${encodeURIComponent(token)}`);
    token = '';

    await new Promise((resolve, reject) => {
      const cleanup = () => {
        clearTimeout(timer);
        ws.removeEventListener('open', onOpen);
        ws.removeEventListener('error', onError);
        ws.removeEventListener('close', onClose);
      };
      const onOpen = () => {
        cleanup();
        resolve();
      };
      const onError = () => {
        cleanup();
        reject(new Error('Hermes WebSocket connection failed'));
      };
      const onClose = (ev) => {
        cleanup();
        reject(new Error(`Hermes WebSocket closed during handshake (${ev.code || 0})`));
      };
      const timer = setTimeout(() => {
        cleanup();
        try {
          ws.close();
        } catch {
          // ignore
        }
        reject(new Error('Hermes WebSocket connection timeout'));
      }, config.connectTimeoutMs);
      if (typeof timer.unref === 'function') timer.unref();

      ws.addEventListener('open', onOpen, { once: true });
      ws.addEventListener('error', onError, { once: true });
      ws.addEventListener('close', onClose, { once: true });
    });

    if (shuttingDown) {
      try {
        ws.close(1000, 'Client shutdown');
      } catch {
        // ignore
      }
      throw new Error('Shutdown in progress');
    }

    ws.addEventListener('message', (event) => {
      if (shuttingDown) return;
      let raw;
      if (typeof event.data === 'string') {
        raw = event.data;
      } else if (event.data instanceof ArrayBuffer) {
        raw = Buffer.from(event.data).toString('utf8');
      } else if (ArrayBuffer.isView(event.data)) {
        raw = Buffer.from(event.data.buffer, event.data.byteOffset, event.data.byteLength).toString('utf8');
      } else {
        raw = String(event.data ?? '');
      }

      let message;
      try {
        message = JSON.parse(raw);
      } catch {
        return;
      }
      if (!message || typeof message !== 'object' || Array.isArray(message)) {
        return;
      }

      // Ignore server notifications (no id) and server->client requests (have method, e.g. approvals)
      if (typeof message.method === 'string' || message.id === undefined || message.id === null) {
        return;
      }

      const key = idKey(message.id);
      const item = pending.get(key);
      if (!item) {
        return;
      }
      pending.delete(key);
      clearTimeout(item.timer);

      if (message.error !== undefined && message.error !== null) {
        if (typeof message.error === 'object' && !Array.isArray(message.error)) {
          writeErrorResponse(
            item.id,
            typeof message.error.code === 'number' ? message.error.code : -32000,
            message.error.message || 'Hermes RPC rejected',
            message.error.data
          );
        } else {
          writeErrorResponse(item.id, -32000, String(message.error));
        }
      } else {
        writeJsonLine({ id: item.id, result: message.result ?? null });
      }
      checkEofDrainComplete();
    });

    ws.addEventListener('close', () => {
      if (shuttingDown) return;
      socket = null;
      connectPromise = null;
      clearPending('Hermes WebSocket closed before response');
    });

    ws.addEventListener('error', () => {
      if (shuttingDown) return;
      clearPending('Hermes WebSocket error');
    });

    socket = ws;
    return ws;
  }

  function ensureConnected() {
    if (socket && socket.readyState === WebSocket.OPEN) {
      return Promise.resolve(socket);
    }
    if (!connectPromise) {
      connectPromise = establishConnection().catch((err) => {
        connectPromise = null;
        throw err;
      });
    }
    return connectPromise;
  }

  function dispatchOverSocket(ws, req) {
    if (shuttingDown) return;
    const key = idKey(req.id);
    if (pending.has(key)) {
      writeErrorResponse(req.id, -32600, 'Duplicate in-flight request id');
      checkEofDrainComplete();
      return;
    }

    const timer = setTimeout(() => {
      pending.delete(key);
      writeErrorResponse(req.id, -32000, 'Hermes RPC timeout');
      checkEofDrainComplete();
    }, config.requestTimeoutMs);

    pending.set(key, { id: req.id, timer });

    try {
      ws.send(
        JSON.stringify({
          jsonrpc: '2.0',
          id: req.id,
          method: req.method,
          params: req.params,
        })
      );
    } catch (err) {
      pending.delete(key);
      clearTimeout(timer);
      writeErrorResponse(req.id, -32001, `Hermes WebSocket send failed: ${sanitizeText(err.message)}`);
      checkEofDrainComplete();
    }
  }

  function handleRequestObject(rawObj) {
    if (!rawObj || typeof rawObj !== 'object' || Array.isArray(rawObj)) {
      writeErrorResponse(null, -32600, 'Request must be a JSON object');
      return;
    }

    const { id, method, params } = rawObj;
    const validId = typeof id === 'string' || (typeof id === 'number' && Number.isFinite(id));
    if (!validId) {
      writeErrorResponse(null, -32600, 'Request id must be a string or finite number');
      return;
    }

    if (typeof method !== 'string' || !method.trim()) {
      writeErrorResponse(id, -32600, 'Request method must be a non-empty string');
      return;
    }

    if (!ALLOWED_METHODS.has(method)) {
      writeErrorResponse(id, -32601, `Method not allowed: ${method}`);
      return;
    }

    let normalizedParams = {};
    if (params !== undefined && params !== null) {
      if (typeof params !== 'object' || Array.isArray(params)) {
        writeErrorResponse(id, -32602, 'Request params must be a JSON object');
        return;
      }
      normalizedParams = params;
    }

    const req = { id, method, params: normalizedParams };

    if (socket && socket.readyState === WebSocket.OPEN) {
      dispatchOverSocket(socket, req);
      return;
    }

    preConnectQueue.push(req);
    if (preConnectQueue.length === 1) {
      ensureConnected()
        .then((ws) => {
          while (preConnectQueue.length > 0 && !shuttingDown) {
            const nextReq = preConnectQueue.shift();
            dispatchOverSocket(ws, nextReq);
          }
          checkEofDrainComplete();
        })
        .catch((err) => {
          clearPreConnectQueue(err.message || 'Hermes connection failed');
        });
    }
  }

  function handleLine(line) {
    const trimmed = line.trim();
    if (!trimmed) return;
    let parsed;
    try {
      parsed = JSON.parse(trimmed);
    } catch {
      writeErrorResponse(null, -32700, 'Invalid JSON line');
      return;
    }
    handleRequestObject(parsed);
  }

  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (chunk) => {
    if (shuttingDown) return;
    stdinBuffer += chunk;
    let newlineIndex;
    while ((newlineIndex = stdinBuffer.indexOf('\n')) !== -1) {
      const line = stdinBuffer.slice(0, newlineIndex).replace(/\r$/, '');
      stdinBuffer = stdinBuffer.slice(newlineIndex + 1);
      handleLine(line);
    }
  });

  process.stdin.on('end', () => {
    if (!shuttingDown && stdinBuffer.trim()) {
      const tail = stdinBuffer.replace(/\r$/, '');
      stdinBuffer = '';
      handleLine(tail);
    }
    stdinEnded = true;
    if (preConnectQueue.length === 0 && pending.size === 0) {
      initiateShutdown(0);
    } else {
      eofDrainTimer = setTimeout(() => {
        eofDrainTimer = null;
        initiateShutdown(0);
      }, EOF_DRAIN_TIMEOUT_MS);
      if (typeof eofDrainTimer.unref === 'function') eofDrainTimer.unref();
    }
  });

  process.stdin.on('error', () => {
    initiateShutdown(1);
  });

  process.once('SIGINT', () => initiateShutdown(0));
  process.once('SIGTERM', () => initiateShutdown(0));
  process.once('uncaughtException', (err) => {
    process.stderr.write(`Uncaught exception: ${sanitizeText(err && err.message ? err.message : err)}\n`);
    initiateShutdown(1);
  });
  process.once('unhandledRejection', (reason) => {
    process.stderr.write(`Unhandled rejection: ${sanitizeText(reason && reason.message ? reason.message : reason)}\n`);
    initiateShutdown(1);
  });
}

main();

