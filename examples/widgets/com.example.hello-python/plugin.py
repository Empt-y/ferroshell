"""A Ferroshell plugin in plain Python (no dependencies). See docs/plugin-protocol.md.

Publishes the time every second for all instances, and each instance's greeting.
"""
import json
import sys
import threading
import time

lock = threading.Lock()


def send(msg):
    with lock:
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def greet(instances):
    for inst in instances:
        greeting = inst.get("settings", {}).get("greeting", "Hello")
        notify("publish", {"instance": inst["instance"], "name": "greeting", "value": greeting})


def ticker():
    while True:
        notify("publish", {"name": "time", "value": time.strftime("%H:%M:%S")})
        time.sleep(1)


for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if "id" in msg and method is not None:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": "ok"})
        if method == "initialize":
            greet(msg["params"].get("instances", []))
            threading.Thread(target=ticker, daemon=True).start()
            notify("log", {"level": "info", "message": "hello-python started"})
    elif method == "instances":
        greet(msg["params"].get("instances", []))
    elif method == "invoke":
        notify("log", {"level": "info", "message": f"clicked: {msg['params']}"})
    elif method == "shutdown":
        break
