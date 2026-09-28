import json, os, socket, sys
path = "/tmp/kazoo/kazoo-wall.sock"
s = socket.socket(socket.AF_UNIX); s.connect(path); f = s.makefile("rw")
n = 0
def call(op, **kw):
    global n; n += 1
    f.write(json.dumps({"id": n, "op": op, **kw}) + "\n"); f.flush()
    while True:
        line = json.loads(f.readline())
        if line.get("id") == n: return line
call("hello", seat="Cassio", client="cassio-shell")
args = sys.argv[1:]
if args:
    print(json.dumps(call(args[0], **json.loads(args[1]) if len(args) > 1 else {})))
