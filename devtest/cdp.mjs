// Talks to the WebView2 of the desktop test build over its debugging port. Only what cannot be
// done from inside the page: a picture of it, also while its window is off-screen.
//
//   node devtest/cdp.mjs <port> shot <file.png>
import { writeFileSync } from 'node:fs';

const [port, command, file] = process.argv.slice(2);

async function page() {
    for (let i = 0; i < 100; i++) {
        try {
            const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
            const found = list.find((target) => target.type === 'page');
            if (found) return found;
        } catch {
            // Not up yet.
        }
        await new Promise((done) => setTimeout(done, 100));
    }
    throw new Error(`no page on port ${port}`);
}

function connect(url) {
    return new Promise((resolve, reject) => {
        const socket = new WebSocket(url);
        socket.onopen = () => resolve(socket);
        socket.onerror = () => reject(new Error(`no connection to ${url}`));
    });
}

function call(socket, method, params = {}) {
    return new Promise((resolve, reject) => {
        const id = Math.floor(Math.random() * 1e9);
        const listener = (event) => {
            const message = JSON.parse(event.data);
            if (message.id !== id) return;
            socket.removeEventListener('message', listener);
            if (message.error) reject(new Error(JSON.stringify(message.error)));
            else resolve(message.result);
        };
        socket.addEventListener('message', listener);
        socket.send(JSON.stringify({ id, method, params }));
    });
}

const socket = await connect((await page()).webSocketDebuggerUrl);
if (command === 'shot') {
    const { data } = await call(socket, 'Page.captureScreenshot', { format: 'png', fromSurface: true });
    writeFileSync(file, Buffer.from(data, 'base64'));
} else {
    throw new Error(`unknown command ${command}`);
}
socket.close();
