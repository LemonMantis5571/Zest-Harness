// Exercise the packaged web windows against a private offline page.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";

const binary = path.resolve(process.argv[2]);
const server = createServer((_, response) => {
  response.setHeader("Content-Type", "text/html; charset=utf-8");
  response.end('<title>Local fixture</title><p>Browser smoke fixture</p>');
});
server.listen(0, "127.0.0.1");
await once(server, "listening");
const url = `http://127.0.0.1:${server.address().port}/`;

async function check(flag, run) {
  const profile = await mkdtemp(path.join(tmpdir(), "zest-web-check-"));
  const child = spawn(binary, [flag], {cwd: profile, windowsHide: true, stdio: ["pipe", "pipe", "inherit"]});
  const done = once(child, "exit");
  const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  const timer = setTimeout(() => child.kill(), 30000);
  async function request(message) {
    child.stdin.write(JSON.stringify(message) + "\n");
    const line = await lines.next();
    assert.equal(line.done, false, "web window closed before acknowledging the request");
    const reply = JSON.parse(line.value);
    assert.ok("Ok" in reply, reply.Err);
    return reply.Ok;
  }
  try {
    await run(request);
    child.stdin.end();
    const [status] = await done;
    assert.equal(status, 0, "web window should close on EOF");
  } finally {
    child.kill();
    await done;
    clearTimeout(timer);
    await rm(profile, {recursive: true, force: true, maxRetries: 5, retryDelay: 150});
  }
}

try {
  await check("--zest-browser-window", async request => {
    assert.equal((await request({action:"open", url})).url, url);
    assert.match(JSON.stringify(await request({action:"snapshot"})), /Browser smoke fixture/);
  });
  if (process.platform === "win32") {
    await check("--zest-artifact-window", request => request({title:"Local document", html:"<button>Count</button>"}));
  }
  console.log("Native web window smoke passed.");
} finally {
  server.close();
}
