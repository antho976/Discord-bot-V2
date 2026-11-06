import sys, time, glob, urllib.request, urllib.parse, os
from multiprocessing import Process, Queue
port, folder, secs = sys.argv[1], sys.argv[2], 12
COOKIE = open(sys.argv[3]).read().strip() if len(sys.argv) > 3 else ""
URL = f"http://localhost:{port}/p/1100000000000000001/idleon"
def worker(i, c, stop, q):
    bodies = [b"save=" + open(f, "rb").read() + b"\r\n" for f in sorted(glob.glob(folder + "/*.json"))]
    lats, busy, k = [], 0, i
    while time.time() < stop:
        req = urllib.request.Request(URL, data=bodies[k % len(bodies)], headers={"Origin": f"http://localhost:{port}", "Content-Type": "text/plain;charset=UTF-8", "Accept-Encoding": "gzip", "Cookie": COOKIE})
        t = time.perf_counter()
        with urllib.request.urlopen(req, timeout=60) as r:
            out = r.read()
            if r.headers.get("Content-Encoding") == "gzip":
                import gzip; out = gzip.decompress(out)
        dt = time.perf_counter() - t; k += c
        if b"lot of saves are being read" in out: busy += 1
        else: lats.append(dt)
    q.put((lats, busy))
def pct(v, p): v = sorted(v); return v[min(len(v) - 1, int(len(v) * p))] if v else 0
if __name__ == "__main__":
    pid = int(os.popen(f"ss -ltnp | grep ':{port} ' | grep -o 'pid=[0-9]*' | cut -d= -f2").read().strip())
    def stat():
        s = open(f"/proc/{pid}/stat").read().split(); return (int(s[13]) + int(s[14])) / os.sysconf("SC_CLK_TCK")
    for c in [1, 2, 8]:
        q = Queue(); stop = time.time() + 2 + secs
        ps = [Process(target=worker, args=(i, c, stop, q)) for i in range(c)]
        [p.start() for p in ps]; time.sleep(2); cpu0 = stat(); t0 = time.time()
        res = [q.get() for _ in ps]; [p.join() for p in ps]
        el = time.time() - t0; cpu = stat() - cpu0
        lats = [x for l, _ in res for x in l]; busy = sum(b for _, b in res)
        hwm = [l for l in open(f"/proc/{pid}/status") if l.startswith("VmHWM")][0].split()[1]
        print(f"{c:>2} at once: {len(lats)/(el+2):6.1f} reviews/s, p50 {pct(lats,.5)*1000:4.0f} ms, p95 {pct(lats,.95)*1000:4.0f} ms, told to retry {busy}, bot CPU {cpu/el:4.2f} cores, peak RSS {int(hwm)//1024} MB")
