'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const crypto = require('node:crypto');
const path = require('node:path');
const { spawn } = require('node:child_process');

const DESKTOP_RPC_PATH = path.join(__dirname, 'desktop_rpc.cjs');
const WS_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

const processAuditLog = [];

function isProcessAlive(pid) {
  if (!pid || typeof pid !== 'number') return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return err && err.code === 'EPERM';
  }
}

function encodeWsTextFrame(text) {
  const payload = Buffer.from(text, 'utf8');
  assert.ok(payload.length < 126, 'Test frame payload must be < 126 bytes');
  return Buffer.concat([Buffer.from([0x81, payload.length]), payload]);
}

function attachMinimalWsFrameReader(socket, onTextMessage, onCloseFrame) {
  let buf = Buffer.alloc(0);
  socket.on('data', (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    while (buf.length >= 2) {
      const opcode = buf[0] & 0x0f;
      const masked = (buf[1] & 0x80) !== 0;
      let length = buf[1] & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (buf.length < 4) return;
        length = buf.readUInt16BE(2);
        offset = 4;
      }
      const maskLength = masked ? 4 : 0;
      if (buf.length < offset + maskLength + length) return;
      const mask = masked ? buf.subarray(offset, offset + 4) : null;
      offset += maskLength;
      const payload = Buffer.from(buf.subarray(offset, offset + length));
      buf = buf.subarray(offset + length);
      if (masked && mask) {
        for (let i = 0; i < payload.length; i += 1) {
          payload[i] ^= mask[i & 3];
        }
      }
      if (opcode === 0x8) {
        try {
          socket.write(Buffer.from([0x88, 0x00]));
          socket.end();
        } catch {
          // ignore
        }
        onCloseFrame();
        return;
      }
      if (opcode === 0x1) {
        onTextMessage(payload.toString('utf8'));
      }
    }
  });
}

function spawnTrackedRpc(args) {
  const record = {
    path: process.execPath,
    script: DESKTOP_RPC_PATH,
    args,
    pid: null,
    startedAt: new Date().toISOString(),
    exitedAt: null,
    exitCode: null,
    signal: null,
    aliveAfterExit: null,
  };
  const child = spawn(process.execPath, [DESKTOP_RPC_PATH, ...args], {
    cwd: __dirname,
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  });
  record.pid = child.pid;
  processAuditLog.push(record);

  let stdoutBuffer = '';
  let allStdout = '';
  let allStderr = '';
  const lineQueue = [];
  const lineWaiters = [];

  child.stdout.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    allStdout += chunk;
    stdoutBuffer += chunk;
    let idx;
    while ((idx = stdoutBuffer.indexOf('\n')) !== -1) {
      const line = stdoutBuffer.slice(0, idx).replace(/\r$/, '');
      stdoutBuffer = stdoutBuffer.slice(idx + 1);
      if (!line.trim()) continue;
      if (lineWaiters.length > 0) {
        const waiter = lineWaiters.shift();
        waiter.resolve(JSON.parse(line));
      } else {
        lineQueue.push(JSON.parse(line));
      }
    }
  });

  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    allStderr += chunk;
  });

  const exitPromise = new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`Subprocess PID=${record.pid} did not exit naturally within timeout`));
    }, 4000);
    if (typeof timer.unref === 'function') timer.unref();

    child.once('exit', (code, signal) => {
      clearTimeout(timer);
      record.exitedAt = new Date().toISOString();
      record.exitCode = code;
      record.signal = signal;
      record.aliveAfterExit = isProcessAlive(record.pid);
      resolve({ code, signal });
    });
  });

  function nextJsonLine(timeoutMs = 2000) {
    if (lineQueue.length > 0) {
      return Promise.resolve(lineQueue.shift());
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const idx = lineWaiters.findIndex((w) => w.resolve === onResolve);
        if (idx !== -1) lineWaiters.splice(idx, 1);
        reject(new Error('Timed out waiting for JSONL response line from desktop_rpc.cjs'));
      }, timeoutMs);
      const onResolve = (val) => {
        clearTimeout(timer);
        resolve(val);
      };
      lineWaiters.push({ resolve: onResolve });
    });
  }

  return {
    child,
    record,
    exitPromise,
    nextJsonLine,
    getOutput: () => ({ stdout: allStdout, stderr: allStderr }),
  };
}

test('desktop_rpc.cjs reuses single WS across multiple RPCs, frames JSONL, ignores server push/requests, handles errors/timeouts, and exits cleanly on stdin EOF', async () => {
  const secretToken = `hermes-secret-${crypto.randomUUID()}`;
  let httpRootHits = 0;
  let wsUpgradeHits = 0;
  let clientClosedWs = false;
  const receivedFrames = [];
  const activeSockets = new Set();
  let notifyPendingOnEofReceived = null;

  const server = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/') {
      httpRootHits += 1;
      res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
      res.end(`<!doctype html><script>window.__HERMES_SESSION_TOKEN__ = ${JSON.stringify(secretToken)};</script>`);
      return;
    }
    res.writeHead(404);
    res.end();
  });

  server.on('upgrade', (req, socket) => {
    wsUpgradeHits += 1;
    activeSockets.add(socket);
    socket.on('close', () => activeSockets.delete(socket));

    const reqUrl = new URL(req.url, 'http://127.0.0.1');
    if (reqUrl.pathname !== '/api/ws' || reqUrl.searchParams.get('token') !== secretToken) {
      socket.write('HTTP/1.1 401 Unauthorized\r\n\r\n');
      socket.destroy();
      return;
    }

    const key = req.headers['sec-websocket-key'];
    const accept = crypto.createHash('sha1').update(key + WS_GUID).digest('base64');
    socket.write(
      [
        'HTTP/1.1 101 Switching Protocols',
        'Upgrade: websocket',
        'Connection: Upgrade',
        `Sec-WebSocket-Accept: ${accept}`,
        '\r\n',
      ].join('\r\n')
    );

    attachMinimalWsFrameReader(
      socket,
      (rawText) => {
        const frame = JSON.parse(rawText);
        receivedFrames.push(frame);

        if (frame.id === 'rpc-1' && frame.method === 'projects.list') {
          // 1) Push notification without id (must be ignored by helper)
          socket.write(
            encodeWsTextFrame(
              JSON.stringify({
                jsonrpc: '2.0',
                method: 'session.event',
                params: { type: 'delta', text: 'push-noise' },
              })
            )
          );
          // 2) Server->client approval request with colliding id (must be ignored, never auto-approved)
          socket.write(
            encodeWsTextFrame(
              JSON.stringify({
                jsonrpc: '2.0',
                id: 'rpc-1',
                method: 'approval.request',
                params: { request_id: 'apr-1' },
              })
            )
          );
          // 3) Actual RPC response for rpc-1
          socket.write(
            encodeWsTextFrame(
              JSON.stringify({
                jsonrpc: '2.0',
                id: 'rpc-1',
                result: { projects: [{ id: 'proj-1', name: 'Agent HUD' }] },
              })
            )
          );
          return;
        }

        if (frame.id === 2 && frame.method === 'session.active_list') {
          socket.write(
            encodeWsTextFrame(
              JSON.stringify({
                jsonrpc: '2.0',
                id: 2,
                result: { sessions: [{ id: 'sess-live-1', status: 'idle' }] },
              })
            )
          );
          return;
        }

        if (frame.id === 'rpc-err' && frame.method === 'projects.get') {
          socket.write(
            encodeWsTextFrame(
              JSON.stringify({
                jsonrpc: '2.0',
                id: 'rpc-err',
                error: { code: 4040, message: 'Project missing' },
              })
            )
          );
          return;
        }

        if (frame.id === 'rpc-timeout' && frame.method === 'projects.for_cwd') {
          // Intentionally do not reply so helper triggers per-request timeout
          return;
        }

        if (frame.id === 'pending-eof' && frame.method === 'projects.project_sessions') {
          if (notifyPendingOnEofReceived) notifyPendingOnEofReceived();
        }
      },
      () => {
        clientClosedWs = true;
      }
    );
  });

  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  const proc = spawnTrackedRpc([
    '--endpoint',
    `http://127.0.0.1:${port}`,
    '--connect-timeout-ms',
    '2000',
    '--request-timeout-ms',
    '180',
  ]);

  try {
    // Split first request across two writes to verify partial JSONL framing
    proc.child.stdin.write('{"id":"rpc-1","method":"projects.li');
    proc.child.stdin.write('st","params":{}}\n');

    const res1 = await proc.nextJsonLine();
    assert.deepEqual(res1, {
      id: 'rpc-1',
      result: { projects: [{ id: 'proj-1', name: 'Agent HUD' }] },
    });

    // Write two requests in a single chunk (multi-line framing) + reuse the same WS connection
    proc.child.stdin.write(
      [
        JSON.stringify({ id: 2, method: 'session.active_list', params: {} }),
        JSON.stringify({ id: 'rpc-err', method: 'projects.get', params: { id: 'missing' } }),
        '',
      ].join('\n')
    );

    const res2 = await proc.nextJsonLine();
    assert.deepEqual(res2, {
      id: 2,
      result: { sessions: [{ id: 'sess-live-1', status: 'idle' }] },
    });

    const resErr = await proc.nextJsonLine();
    assert.deepEqual(resErr, {
      id: 'rpc-err',
      error: { code: 4040, message: 'Project missing' },
    });

    // Verify single HTTP token fetch and single WebSocket connection reused across RPCs
    assert.equal(httpRootHits, 1);
    assert.equal(wsUpgradeHits, 1);

    // Disallowed method must be rejected locally without sending over WS
    proc.child.stdin.write(`${JSON.stringify({ id: 'forbidden-1', method: 'config.set', params: {} })}\n`);
    const resForbidden = await proc.nextJsonLine();
    assert.equal(resForbidden.id, 'forbidden-1');
    assert.equal(resForbidden.error.code, -32601);

    // Per-request timeout test
    proc.child.stdin.write(
      `${JSON.stringify({ id: 'rpc-timeout', method: 'projects.for_cwd', params: { cwd: 'Z:\\Agent HUD' } })}\n`
    );
    const resTimeout = await proc.nextJsonLine(1500);
    assert.equal(resTimeout.id, 'rpc-timeout');
    assert.equal(resTimeout.error.code, -32000);

    // Send one more request that remains in-flight, then trigger stdin EOF
    const serverSawPendingEof = new Promise((resolve) => {
      notifyPendingOnEofReceived = resolve;
    });
    proc.child.stdin.write(
      `${JSON.stringify({ id: 'pending-eof', method: 'projects.project_sessions', params: { project_id: 'proj-1' } })}\n`
    );
    await serverSawPendingEof;

    // Trigger stdin EOF; child must clear pending timer, close socket, and exit naturally
    proc.child.stdin.end();
    const exitInfo = await proc.exitPromise;
    assert.equal(exitInfo.code, 0);
    assert.equal(exitInfo.signal, null);
    assert.equal(clientClosedWs, true);

    // Verify server never received forbidden method or any auto-approval frame
    assert.deepEqual(
      receivedFrames.map((f) => f.method),
      ['projects.list', 'session.active_list', 'projects.get', 'projects.for_cwd', 'projects.project_sessions']
    );
  } finally {
    if (!proc.child.stdin.destroyed) {
      proc.child.stdin.end();
    }
    await proc.exitPromise;
    assert.equal(proc.record.aliveAfterExit, false, 'Subprocess must not remain alive after exit');

    for (const sock of activeSockets) {
      sock.destroy();
    }
    await new Promise((resolve) => server.close(resolve));

    const { stdout, stderr } = proc.getOutput();
    assert.equal(stdout.includes(secretToken), false, 'Token must never appear in stdout');
    assert.equal(stderr.includes(secretToken), false, 'Token must never appear in stderr');
  }
});

test('desktop_rpc.cjs enforces loopback HTTP endpoint and handles handshake rejection without leaking token', async () => {
  // 1) Reject non-loopback endpoint
  const nonLoopback = spawnTrackedRpc(['--endpoint', 'http://192.168.1.25:62322']);
  try {
    nonLoopback.child.stdin.end();
    const exit1 = await nonLoopback.exitPromise;
    assert.equal(exit1.code, 1);
  } finally {
    assert.equal(nonLoopback.record.aliveAfterExit, false);
  }

  // 2) Reject non-http scheme
  const nonHttp = spawnTrackedRpc(['--endpoint', 'ws://127.0.0.1:62322']);
  try {
    nonHttp.child.stdin.end();
    const exit2 = await nonHttp.exitPromise;
    assert.equal(exit2.code, 1);
  } finally {
    assert.equal(nonHttp.record.aliveAfterExit, false);
  }

  // 3) Handshake rejection / HTTP error reports JSONL error and never leaks token
  const leakTestToken = `hermes-secret-${crypto.randomUUID()}`;
  const rejectServer = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(`<script>window.__HERMES_SESSION_TOKEN__ = "${leakTestToken}";</script>`);
  });
  rejectServer.on('upgrade', (_req, socket) => {
    socket.write('HTTP/1.1 401 Unauthorized\r\n\r\n');
    socket.destroy();
  });

  await new Promise((resolve) => rejectServer.listen(0, '127.0.0.1', resolve));
  const { port } = rejectServer.address();
  const proc = spawnTrackedRpc(['--endpoint', `http://127.0.0.1:${port}`, '--connect-timeout-ms', '1000']);

  try {
    proc.child.stdin.write(`${JSON.stringify({ id: ' handshake-fail', method: 'projects.list', params: {} })}\n`);
    const errLine = await proc.nextJsonLine();
    assert.equal(errLine.id, ' handshake-fail');
    assert.equal(errLine.error.code, -32001);
    proc.child.stdin.end();
    await proc.exitPromise;
  } finally {
    if (!proc.child.stdin.destroyed) {
      proc.child.stdin.end();
    }
    await proc.exitPromise;
    assert.equal(proc.record.aliveAfterExit, false);
    await new Promise((resolve) => rejectServer.close(resolve));

    const { stdout, stderr } = proc.getOutput();
    assert.equal(stdout.includes(leakTestToken), false);
    assert.equal(stderr.includes(leakTestToken), false);
  }

  console.log('PROCESS_AUDIT_SUMMARY=' + JSON.stringify(processAuditLog));
});
