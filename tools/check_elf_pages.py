#!/usr/bin/env python3
"""Check ELF64 PT_LOAD layout against a requested Linux base page size."""

from __future__ import annotations

import argparse
import struct
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

PT_LOAD = 1
EM_X86_64 = 62
EM_AARCH64 = 183
MACHINE_NAMES = {
    EM_X86_64: "x86_64",
    EM_AARCH64: "AArch64",
}


@dataclass(frozen=True)
class LoadSegment:
    index: int
    offset: int
    virtual_address: int
    alignment: int


@dataclass(frozen=True)
class ElfImage:
    bits: int
    byte_order: str
    machine: int
    segments: tuple[LoadSegment, ...]


def parse_elf(data: bytes) -> ElfImage:
    if len(data) < 64 or data[:4] != b"\x7fELF":
        raise ValueError("not an ELF file or truncated ELF64 header")
    if data[4] != 2:
        raise ValueError("only ELF64 files are supported")
    if data[5] == 1:
        byte_order = "little"
        endian = "<"
    elif data[5] == 2:
        byte_order = "big"
        endian = ">"
    else:
        raise ValueError(f"unsupported ELF byte order {data[5]}")

    machine = struct.unpack_from(endian + "H", data, 18)[0]
    phoff = struct.unpack_from(endian + "Q", data, 32)[0]
    phentsize, phnum = struct.unpack_from(endian + "HH", data, 54)
    expected_phentsize = 56
    if phentsize < expected_phentsize:
        raise ValueError(f"program header entry size {phentsize} is below ELF64 minimum")
    if phoff > len(data) or phnum > (len(data) - phoff) // phentsize:
        raise ValueError("program header table extends beyond the file")

    segments: list[LoadSegment] = []
    for index in range(phnum):
        start = phoff + index * phentsize
        p_type, _flags, p_offset, p_vaddr, _paddr, _filesz, _memsz, p_align = (
            struct.unpack_from(endian + "IIQQQQQQ", data, start)
        )
        if p_type == PT_LOAD:
            segments.append(LoadSegment(index, p_offset, p_vaddr, p_align))
    if not segments:
        raise ValueError("ELF has no PT_LOAD segments")
    return ElfImage(64, byte_order, machine, tuple(segments))


def check_image(
    image: ElfImage, page_size: int, expected_machine: int | None = None
) -> list[str]:
    errors: list[str] = []
    if page_size <= 0 or page_size & (page_size - 1):
        return [f"page size {page_size} is not a positive power of two"]
    if expected_machine is not None and image.machine != expected_machine:
        expected = MACHINE_NAMES.get(expected_machine, str(expected_machine))
        actual = MACHINE_NAMES.get(image.machine, str(image.machine))
        errors.append(f"expected machine {expected}, found {actual}")

    for segment in image.segments:
        label = f"LOAD[{segment.index}]"
        alignment = segment.alignment
        if alignment < page_size:
            errors.append(f"{label}: p_align 0x{alignment:x} is below page size 0x{page_size:x}")
        if alignment > 1 and alignment & (alignment - 1):
            errors.append(f"{label}: p_align 0x{alignment:x} is not a power of two")
        if alignment > 1 and (segment.virtual_address - segment.offset) % alignment:
            errors.append(
                f"{label}: p_vaddr and p_offset are not congruent modulo p_align 0x{alignment:x}"
            )
        if (segment.virtual_address - segment.offset) % page_size:
            errors.append(
                f"{label}: p_vaddr and p_offset are not congruent modulo page size 0x{page_size:x}"
            )
    return errors


def describe_image(image: ElfImage, page_size: int) -> str:
    machine = MACHINE_NAMES.get(image.machine, f"machine-{image.machine}")
    lines = [f"ELF{image.bits} {machine}; page size {page_size}"]
    for segment in image.segments:
        lines.append(
            f"  LOAD[{segment.index}] p_align=0x{segment.alignment:x} "
            f"p_offset%page=0x{segment.offset % page_size:x} "
            f"p_vaddr%page=0x{segment.virtual_address % page_size:x}"
        )
    return "\n".join(lines)


def positive_int(value: str) -> int:
    try:
        parsed = int(value, 0)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if parsed <= 0 or parsed & (parsed - 1):
        raise argparse.ArgumentTypeError("must be a positive power of two")
    return parsed


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--page-size",
        required=True,
        type=positive_int,
        help="target Linux base page size in bytes (for example 16384)",
    )
    parser.add_argument(
        "--machine",
        choices=("any", "aarch64", "x86_64"),
        default="any",
        help="optionally require an ELF machine",
    )
    parser.add_argument("elf", nargs="+", type=Path, help="ELF64 files to check")
    args = parser.parse_args(argv)
    expected_machine = {
        "aarch64": EM_AARCH64,
        "x86_64": EM_X86_64,
    }.get(args.machine)

    failed = False
    for path in args.elf:
        try:
            image = parse_elf(path.read_bytes())
        except (OSError, ValueError) as error:
            print(f"FAIL {path}: {error}", file=sys.stderr)
            failed = True
            continue
        print(f"{path}:\n{describe_image(image, args.page_size)}")
        errors = check_image(image, args.page_size, expected_machine)
        if errors:
            for error in errors:
                print(f"  FAIL: {error}", file=sys.stderr)
            failed = True
        else:
            print("  PASS: all PT_LOAD segments satisfy page layout checks")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
