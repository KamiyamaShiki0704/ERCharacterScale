"""Check an x64 PE's normal and delayed imports using only Python's stdlib.

Usage: python tools/check_runtime_deps.py path/to/ERCharacterScale.dll
Any dependency outside the explicit Windows system list fails the release check.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys


SYSTEM_DLLS = {
    "kernel32.dll",
    "ntdll.dll",
    "oleaut32.dll",
    "bcryptprimitives.dll",
    "api-ms-win-core-synch-l1-2-0.dll",
}


class PE:
    def __init__(self, data):
        self.data = data
        if data[:2] != b"MZ":
            raise ValueError("missing DOS header")
        header = self.unpack("<I", 0x3C)[0]
        if data[header:header + 4] != b"PE\0\0":
            raise ValueError("missing PE signature")
        machine, count = self.unpack("<HH", header + 4)
        optional_size = self.unpack("<H", header + 20)[0]
        optional = header + 24
        if machine != 0x8664 or self.unpack("<H", optional)[0] != 0x20B:
            raise ValueError("expected Windows x64 PE32+")
        if optional_size < 112 or optional + optional_size > len(data):
            raise ValueError("invalid optional header")
        self.image_base = self.unpack("<Q", optional + 24)[0]
        self.header_size = self.unpack("<I", optional + 60)[0]
        self.directory_count = self.unpack("<I", optional + 108)[0]
        if self.directory_count > (optional_size - 112) // 8:
            raise ValueError("invalid data directory count")
        self.directories = optional + 112
        self.sections = []
        start = optional + optional_size
        for i in range(count):
            virtual_size, rva, raw_size, offset = self.unpack("<IIII", start + i * 40 + 8)
            if offset + raw_size > len(data):
                raise ValueError("section extends outside file")
            self.sections.append((rva, raw_size, offset))

    def unpack(self, fmt, offset):
        if offset < 0 or offset + struct.calcsize(fmt) > len(self.data):
            raise ValueError("truncated PE structure")
        return struct.unpack_from(fmt, self.data, offset)

    def offset(self, rva, size=1):
        if 0 <= rva < self.header_size and rva + size <= min(self.header_size, len(self.data)):
            return rva
        for start, raw_size, offset in self.sections:
            if start <= rva and rva + size <= start + raw_size:
                return offset + rva - start
        raise ValueError(f"unmapped RVA 0x{rva:x}")

    def name(self, rva):
        result = bytearray()
        for i in range(512):
            value = self.data[self.offset(rva + i)]
            if value == 0:
                if not result:
                    raise ValueError("empty import name")
                return result.decode("ascii").lower()
            result.append(value)
        raise ValueError("unterminated import name")

    def imports(self, index, delayed=False):
        if index >= self.directory_count:
            return []
        rva, size = self.unpack("<II", self.directories + index * 8)
        if not rva and not size:
            return []
        stride = 32 if delayed else 20
        if not rva or size < stride:
            raise ValueError("invalid import directory")
        names = []
        for i in range(min(size // stride, 4096)):
            fields = self.unpack("<8I" if delayed else "<5I", self.offset(rva + i * stride, stride))
            if not any(fields):
                return sorted(set(names))
            name_rva = fields[1] if delayed else fields[3]
            if delayed and not fields[0] & 1:
                name_rva -= self.image_base
            names.append(self.name(name_rva))
        raise ValueError("import directory has no bounded terminator")


def inspect(path):
    data = path.read_bytes()
    pe = PE(data)
    normal, delayed = pe.imports(1), pe.imports(13, delayed=True)
    unexpected = sorted((set(normal) | set(delayed)) - SYSTEM_DLLS)
    return {
        "artifact": path.name,
        "sha256": hashlib.sha256(data).hexdigest().upper(),
        "bytes": len(data),
        "imports": normal,
        "delay_imports": delayed,
        "unexpected_dependencies": unexpected,
        "passed": not unexpected,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dll", type=Path)
    args = parser.parse_args()
    try:
        result = inspect(args.dll)
    except (OSError, ValueError, struct.error) as error:
        print(f"PE dependency check failed: {error}", file=sys.stderr)
        return 2
    print(json.dumps(result, indent=2))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
