// Drives the installed production app through its real Settings screen and makes exactly one
// Gemini connectivity request. `network-egress.ps1` observes the app process around this call.
// The API key is read from the environment and is never printed or passed on the command line.

const port = Number(process.env.SURVEY_CDP_PORT ?? "9223");
const key = process.env.GEMINI_API_KEY;
if (!key) throw new Error("GEMINI_API_KEY is required");

const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function pageTarget() {
  const until = Date.now() + 120_000;
  let last = "nothing listening";
  while (Date.now() < until) {
    try {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = targets.find((target) => target.type === "page" && target.webSocketDebuggerUrl);
      if (page) return page;
      last = `targets: ${JSON.stringify(targets.map((target) => target.type))}`;
    } catch (error) {
      last = error.message;
    }
    await sleep(500);
  }
  throw new Error(`no WebView2 page on port ${port}; last: ${last}`);
}

const target = await pageTarget();
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = () => reject(new Error("DevTools WebSocket failed"));
});

let nextId = 1;
const pending = new Map();
socket.onmessage = (event) => {
  const message = JSON.parse(event.data);
  const request = pending.get(message.id);
  if (!request) return;
  pending.delete(message.id);
  if (message.error) request.reject(new Error(message.error.message));
  else request.resolve(message.result);
};
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });

async function evaluate(expression) {
  const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  }
  return result.result.value;
}

async function waitFor(expression, label, timeoutMs = 120_000) {
  const until = Date.now() + timeoutMs;
  while (Date.now() < until) {
    const value = await evaluate(expression);
    if (value) return value;
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${label}`);
}

await send("Page.bringToFront").catch(() => {});
await waitFor(`document.querySelector('#settings-api-key') !== null`, "the Settings API-key field");
await evaluate(`(() => {
  const input = document.querySelector('#settings-api-key');
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  setter.call(input, ${JSON.stringify(key)});
  input.dispatchEvent(new Event('input', { bubbles: true }));
})()`);
await waitFor(
  `[...document.querySelectorAll('button')].some((button) => button.textContent.includes('Save and test') && !button.disabled)`,
  "Save and test to enable",
);
await evaluate(`[...document.querySelectorAll('button')].find((button) => button.textContent.includes('Save and test')).click()`);
const status = await waitFor(
  `(() => {
    const text = [...document.querySelectorAll('[role="status"]')].map((node) => node.textContent).join(' ');
    if (text.includes('Connected:')) return text;
    if (text && !text.includes('Checking the key with Gemini')) return 'ERROR:' + text;
    return '';
  })()`,
  "the Gemini connectivity result",
);
if (status.startsWith("ERROR:")) throw new Error(status.slice(6));

await waitFor(
  `[...document.querySelectorAll('button')].some((button) => button.textContent.includes('Delete key') && !button.disabled)`,
  "Delete key to enable",
);
await evaluate(`[...document.querySelectorAll('button')].find((button) => button.textContent.includes('Delete key')).click()`);
await waitFor(
  `[...document.querySelectorAll('[role="status"]')].some((node) => node.textContent.includes('Key deleted.'))`,
  "credential deletion",
);
console.log("Gemini connectivity request completed and the temporary credential was deleted");
socket.close();

