#!/usr/bin/env python3
"""Generate owned test bytes and serve them over loopback BitTorrent/NNTP fixtures.

No upstream tracker, peer discovery, Usenet account or external content is used. Download
clients are real containers; only their data source is this generated local fixture.
"""
import argparse
import hashlib
import json
from pathlib import Path
import socket
import socketserver
import struct
import sys
import threading
import time
import urllib.parse
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def bencode(value):
    if isinstance(value, int):
        return b"i" + str(value).encode() + b"e"
    if isinstance(value, bytes):
        return str(len(value)).encode() + b":" + value
    if isinstance(value, list):
        return b"l" + b"".join(bencode(v) for v in value) + b"e"
    return b"d" + b"".join(bencode(k) + bencode(value[k]) for k in sorted(value)) + b"e"


def fixture(root, http_port):
    root.mkdir(parents=True, exist_ok=True)
    name = "kitsunarr-client-proof.bin"
    data = b"".join(hashlib.sha256(f"Kitsunarr local proof {i}".encode()).digest() for i in range(65536))
    (root / name).write_bytes(data)
    piece_length = 256 * 1024
    info = {b"name": name.encode(), b"length": len(data), b"piece length": piece_length,
            b"pieces": b"".join(hashlib.sha1(data[n:n + piece_length]).digest() for n in range(0, len(data), piece_length)), b"private": 1}
    torrent = bencode({b"announce": f"http://127.0.0.1:{http_port}/announce".encode(), b"info": info,
                       b"url-list": [f"http://127.0.0.1:{http_port}/proof-data/{name}".encode()]})
    (root / "proof.torrent").write_bytes(torrent)
    chunks = []
    line = bytearray()
    for byte in data:
        encoded = (byte + 42) & 255
        if encoded in (0, 9, 10, 13, 32, 46, 61):
            line.extend((61, (encoded + 64) & 255))
        else:
            line.append(encoded)
        if len(line) >= 128:
            chunks.append(bytes(line))
            line.clear()
    if line:
        chunks.append(bytes(line))
    body = (f"=ybegin line=128 size={len(data)} name={name}\r\n".encode()
            + b"\r\n".join(chunks)
            + f"\r\n=yend size={len(data)} crc32={zlib.crc32(data) & 0xffffffff:08x}\r\n".encode())
    message_id = "kitsunarr-proof@local.invalid"
    headers = (f"From: Kitsunarr Test <proof@local.invalid>\r\nNewsgroups: alt.binaries.test\r\n"
               f'Subject: "{name}" yEnc (1/1)\r\nMessage-ID: <{message_id}>\r\n').encode()
    nzb = (f'<?xml version="1.0"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">'
           f'<file poster="proof@local.invalid" date="{int(time.time())}" subject="&quot;{name}&quot; yEnc (1/1)">'
           f'<groups><group>alt.binaries.test</group></groups><segments>'
           f'<segment bytes="{len(body)}" number="1">{message_id}</segment>'
           '</segments></file></nzb>').encode()
    (root / "proof.nzb").write_bytes(nzb)
    manifest = {"name": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
                "infoHash": hashlib.sha1(bencode(info)).hexdigest().upper()}
    (root / "proof.json").write_text(json.dumps(manifest, indent=2))
    return torrent, nzb, headers, body, message_id


def serve(root, http_port, nntp_port, seed_port):
    torrent, nzb, headers, body, message_id = fixture(root, http_port)
    counters = {"announces": 0, "articles": 0, "bytesServed": 0, "webseedRequests": 0, "webseedBytes": 0}
    lock = threading.Lock()

    def count(name, amount=1):
        with lock:
            counters[name] += amount
            path = root / "source-stats.json"
            temp = path.with_suffix(".tmp")
            temp.write_text(json.dumps(counters, indent=2))
            temp.replace(path)

    class Http(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urllib.parse.urlsplit(self.path).path
            if path == "/proof-data/kitsunarr-client-proof.bin":
                data = (root / "kitsunarr-client-proof.bin").read_bytes()
                start, end = 0, len(data) - 1
                partial = self.headers.get("Range", "")
                if partial.startswith("bytes="):
                    first, _, last = partial[6:].partition("-")
                    start = int(first or 0)
                    end = min(int(last or end), end)
                response = data[start:end + 1]
                count("webseedRequests")
                count("webseedBytes", len(response))
                self.send_response(206 if partial else 200)
                self.send_header("Accept-Ranges", "bytes")
                self.send_header("Content-Length", str(len(response)))
                if partial:
                    self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
                self.end_headers()
                self.wfile.write(response)
                return
            if path == "/announce":
                count("announces")
                response = bencode({b"interval": 5, b"min interval": 1, b"complete": 1, b"incomplete": 0,
                                    b"peers": socket.inet_aton("127.0.0.1") + struct.pack("!H", seed_port)})
            elif path == "/proof.torrent":
                response = torrent
            elif path == "/proof.nzb":
                response = nzb
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Length", str(len(response)))
            self.end_headers()
            self.wfile.write(response)

        def log_message(self, *args):
            pass

    class Nntp(socketserver.StreamRequestHandler):
        def handle(self):
            self.wfile.write(b"200 Kitsunarr local generated proof ready\r\n")
            while raw := self.rfile.readline(65536):
                command, _, argument = raw.decode("ascii", "replace").strip().partition(" ")
                command = command.upper()
                if command == "QUIT":
                    self.wfile.write(b"205 Bye\r\n")
                    return
                if command == "CAPABILITIES":
                    response = b"101 Capabilities\r\nVERSION 2\r\nREADER\r\n.\r\n"
                elif command == "MODE":
                    response = b"200 Reader mode\r\n"
                elif command == "AUTHINFO":
                    response = b"381 Password required\r\n" if argument.upper().startswith("USER") else b"281 Authenticated\r\n"
                elif command == "GROUP":
                    response = b"211 1 1 1 alt.binaries.test\r\n"
                elif command == "DATE":
                    response = ("111 " + time.strftime("%Y%m%d%H%M%S", time.gmtime()) + "\r\n").encode()
                elif command in ("ARTICLE", "BODY", "HEAD", "STAT"):
                    if argument.strip("<>") not in (message_id, "1", ""):
                        response = b"430 No such article\r\n"
                    else:
                        code = {"ARTICLE": 220, "HEAD": 221, "BODY": 222, "STAT": 223}[command]
                        response = f"{code} 1 <{message_id}>\r\n".encode()
                        if command != "STAT":
                            article = headers if command == "HEAD" else body if command == "BODY" else headers + b"\r\n" + body
                            response += b"\r\n".join(b"." + line if line.startswith(b".") else line for line in article.split(b"\r\n")) + b".\r\n"
                            if command != "HEAD":
                                count("articles")
                                count("bytesServed", len(body))
                else:
                    response = b"500 Unsupported local fixture command\r\n"
                self.wfile.write(response)

    class Server(socketserver.ThreadingTCPServer):
        allow_reuse_address = True
        daemon_threads = True

        def handle_error(self, request, client_address):
            if not isinstance(sys.exc_info()[1], (ConnectionResetError, BrokenPipeError)):
                super().handle_error(request, client_address)

    nntp = Server(("127.0.0.1", nntp_port), Nntp)
    threading.Thread(target=nntp.serve_forever, daemon=True).start()
    print(f"Generated proof: {root}; local tracker {http_port}, NNTP {nntp_port}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", http_port), Http).serve_forever()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(".dev/download-clients/proof"))
    parser.add_argument("--http-port", type=int, default=18081)
    parser.add_argument("--nntp-port", type=int, default=11119)
    parser.add_argument("--seed-port", type=int, default=16882)
    args = parser.parse_args()
    serve(args.root.resolve(), args.http_port, args.nntp_port, args.seed_port)
