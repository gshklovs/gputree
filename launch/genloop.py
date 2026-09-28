import json, urllib.request, time
body = json.dumps({"prompt": "Write a long story about a robot duck learning to walk.", "n_predict": 20, "temperature": 0.8}).encode()
while True:
    try:
        urllib.request.urlopen(urllib.request.Request("http://127.0.0.1:8089/completion", body, {"Content-Type": "application/json"}), timeout=60).read()
    except Exception:
        pass
    time.sleep(1.2)
