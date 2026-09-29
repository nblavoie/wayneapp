// Serveur statique minimal pour prévisualiser site-out/ : node tools/serve.js [port]
const http = require("http");
const fs = require("fs");
const path = require("path");

const root = path.join(__dirname, "..", "site-out");
const port = Number(process.argv[2]) || 8765;
const types = { ".html": "text/html; charset=utf-8", ".png": "image/png", ".svg": "image/svg+xml", ".ico": "image/x-icon", ".exe": "application/octet-stream", ".sha256": "text/plain" };

http.createServer((req, res) => {
  const url = decodeURIComponent(req.url.split("?")[0]);
  let file = path.normalize(path.join(root, url.endsWith("/") ? url + "index.html" : url));
  if (!file.startsWith(root)) { res.writeHead(403); return res.end(); }
  fs.readFile(file, (err, data) => {
    if (err) { res.writeHead(404); return res.end("404"); }
    res.writeHead(200, { "Content-Type": types[path.extname(file)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(data);
  });
}).listen(port, () => console.log(`http://localhost:${port}`));
