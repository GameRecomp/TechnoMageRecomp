# SPDX-License-Identifier: MIT
# Test server for the browser player in a headless browser (tools/web-test/run):
#   python3 serve.py WEB_DIR GAME_DIR OUT_DIR PORT
# serves web/ (the player's modules), /t/ (this directory: test.html), /repo/ (the checkout:
# the runtime module), /webgame/ (GAME_DIR: game/… and code.wasm from the converter, with
# ranges) and /index.json (the game's files); POST /log and /<name> land in OUT_DIR.
import http.server, json, os, sys, urllib.parse
SITE, GAME, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
class H(http.server.SimpleHTTPRequestHandler):
    def translate_path(self, path):
        p = urllib.parse.unquote(urllib.parse.urlparse(path).path)
        if p.startswith('/webgame/'):
            return os.path.join(GAME, p[len('/webgame/'):])
        if p.startswith('/orig/'):
            return os.path.join(os.getcwd(), 'orig', p[len('/orig/'):])
        if p.startswith('/t/'):
            return os.path.join(os.path.dirname(os.path.abspath(__file__)), p[3:])
        if p.startswith('/repo/'):
            return os.path.join(os.getcwd(), p[len('/repo/'):])
        return os.path.join(SITE, p.lstrip('/'))
    def log_message(self, *a): pass
    def end_headers(self):
        self.send_header('Cache-Control', 'no-store')
        super().end_headers()
    def do_GET(self):
        if self.path == '/index.json':
            out = []
            root = os.path.join(GAME, 'game')
            for d, dirs, files in os.walk(root):
                rel = os.path.relpath(d, GAME).replace(os.sep, '/')
                out.append([rel, -1])
                for f in files:
                    out.append([f'{rel}/{f}', os.path.getsize(os.path.join(d, f))])
            b = json.dumps(out).encode()
            self.send_response(200); self.send_header('Content-Type', 'application/json'); self.send_header('Content-Length', str(len(b))); self.end_headers(); self.wfile.write(b); return
        rng = self.headers.get('Range')
        path = self.translate_path(self.path)
        if rng and os.path.isfile(path):
            a, b = rng.split('=')[1].split('-')
            size = os.path.getsize(path)
            a = int(a); b = min(int(b) if b else size - 1, size - 1)
            with open(path, 'rb') as f:
                f.seek(a); data = f.read(b - a + 1)
            self.send_response(206)
            self.send_header('Content-Range', f'bytes {a}-{b}/{size}')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers(); self.wfile.write(data); return
        return super().do_GET()
    def do_POST(self):
        n = int(self.headers.get('Content-Length', 0)); data = self.rfile.read(n)
        name = self.path.strip('/').replace('/', '_')
        mode = 'ab' if name == 'log' else 'wb'
        with open(os.path.join(OUT, name), mode) as f:
            f.write(data + (b'\n' if name == 'log' else b''))
        self.send_response(204); self.end_headers()
http.server.ThreadingHTTPServer(('127.0.0.1', int(sys.argv[4])), H).serve_forever()
