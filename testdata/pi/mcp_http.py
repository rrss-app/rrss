#!/usr/bin/env python3
"""Local Streamable HTTP fixture, with an optional fake OAuth authorization server."""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlparse
from mcp_stdio import respond

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def send(self, status, body=None, **headers):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        for key, value in headers.items(): self.send_header(key, value)
        self.end_headers()
        if body is not None: self.wfile.write(json.dumps(body).encode())

    def do_GET(self):
        base = self.server.base
        if self.path.startswith("/.well-known/oauth-protected-resource"):
            self.send(200, {"resource": base + "/mcp", "authorization_servers": [base]})
        elif self.path.startswith("/.well-known/oauth-authorization-server"):
            self.send(200, {"issuer": base, "authorization_endpoint": base + "/authorize", "token_endpoint": base + "/token", "registration_endpoint": base + "/register", "response_types_supported": ["code"], "grant_types_supported": ["authorization_code", "refresh_token"], "code_challenge_methods_supported": ["S256"], "token_endpoint_auth_methods_supported": ["none"]})
        elif self.path.startswith("/authorize"):
            query = parse_qs(urlparse(self.path).query)
            self.send(302, Location=query["redirect_uri"][0] + "?" + urlencode({"code": "fixture-code", "state": query["state"][0]}))
        else: self.send(404)

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path == "/register":
            self.send(201, {**json.loads(raw), "client_id": "rrss-fixture-client", "token_endpoint_auth_method": "none"})
        elif self.path == "/token":
            self.send(200, {"access_token": "rrss-fixture-access", "refresh_token": "rrss-fixture-refresh", "token_type": "Bearer", "expires_in": 3600})
        elif self.path == "/mcp":
            if self.server.oauth and self.headers.get("Authorization") != "Bearer rrss-fixture-access":
                self.send(401, **{"WWW-Authenticate": f'Bearer resource_metadata="{self.server.base}/.well-known/oauth-protected-resource"'})
                return
            result = respond(json.loads(raw))
            if result is None: self.send(202)
            else: self.send(200, result, **{"Mcp-Session-Id": "fixture-session"})
        else: self.send(404)

    def do_DELETE(self):
        self.send(200)

if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.base = f"http://127.0.0.1:{server.server_port}"
    server.oauth = "--oauth" in sys.argv
    print(server.base, flush=True)
    server.serve_forever()
