"""Lee archivos sueltos de un zip remoto grande con peticiones HTTP por rangos.

uso: python -I remotezip.py URL list [patrón]
     python -I remotezip.py URL get DESTINO nombre1 nombre2 ...
     python -I remotezip.py URL getlist DESTINO lista.txt
"""
import io
import os
import sys
import urllib.request
import zipfile


class RangeFile(io.RawIOBase):
    def __init__(self, url):
        self.url = url
        req = urllib.request.Request(url, method="HEAD")
        with urllib.request.urlopen(req) as r:
            self.size = int(r.headers["Content-Length"])
            self.url = r.url
        self.pos = 0
        self.cache = {}

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, off, whence=0):
        self.pos = {0: off, 1: self.pos + off, 2: self.size + off}[whence]
        return self.pos

    def read(self, n=-1):
        if n < 0:
            n = self.size - self.pos
        if n == 0 or self.pos >= self.size:
            return b""
        end = min(self.size, self.pos + n) - 1
        req = urllib.request.Request(self.url, headers={"Range": f"bytes={self.pos}-{end}"})
        for attempt in range(5):
            try:
                with urllib.request.urlopen(req, timeout=120) as r:
                    data = r.read()
                break
            except Exception:
                if attempt == 4:
                    raise
        self.pos += len(data)
        return data

    def readinto(self, b):
        data = self.read(len(b))
        b[: len(data)] = data
        return len(data)


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    url, cmd = sys.argv[1], sys.argv[2]
    def open_zip():
        return zipfile.ZipFile(io.BufferedReader(RangeFile(url), buffer_size=1 << 14))

    z = open_zip()
    if cmd == "list":
        pat = sys.argv[3] if len(sys.argv) > 3 else ""
        for i in z.infolist():
            if pat in i.filename:
                print(i.filename, i.file_size)
        return
    dest = sys.argv[3]
    names = sys.argv[4:]
    if cmd == "getlist":
        names = [l.strip() for l in open(names[0], encoding="utf-8") if l.strip()]
    import threading
    from concurrent.futures import ThreadPoolExecutor

    local = threading.local()

    def get(name):
        out = os.path.join(dest, name.replace("/", os.sep))
        if os.path.exists(out):
            return
        if not hasattr(local, "z"):
            local.z = open_zip()
        os.makedirs(os.path.dirname(out), exist_ok=True)
        with local.z.open(name) as src, open(out + ".part", "wb") as dst:
            dst.write(src.read())
        os.replace(out + ".part", out)

    with ThreadPoolExecutor(8) as pool:
        for k, _ in enumerate(pool.map(get, names)):
            if k % 200 == 0:
                print("ok", k, flush=True)


if __name__ == "__main__":
    main()
