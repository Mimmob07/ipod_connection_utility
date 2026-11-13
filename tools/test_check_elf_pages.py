import struct
import unittest

from check_elf_pages import (
    EM_AARCH64,
    EM_X86_64,
    check_image,
    parse_elf,
)


def synthetic_elf(machine, segments, endian="<"):
    phoff = 64
    phentsize = 56
    data = bytearray(phoff + phentsize * len(segments))
    data[:4] = b"\x7fELF"
    data[4:7] = bytes((2, 1 if endian == "<" else 2, 1))
    struct.pack_into(
        endian + "HHIQQQIHHHHHH",
        data,
        16,
        3,
        machine,
        1,
        0,
        phoff,
        0,
        0,
        64,
        phentsize,
        len(segments),
        64,
        0,
        0,
    )
    for index, (offset, vaddr, alignment) in enumerate(segments):
        struct.pack_into(
            endian + "IIQQQQQQ",
            data,
            phoff + index * phentsize,
            1,
            5,
            offset,
            vaddr,
            0,
            0x1000,
            0x1000,
            alignment,
        )
    return bytes(data)


class CheckElfPagesTests(unittest.TestCase):
    def test_accepts_aarch64_64k_load_segments(self):
        image = parse_elf(synthetic_elf(EM_AARCH64, [(0, 0, 0x10000), (0x20000, 0x20000, 0x10000)]))
        self.assertEqual(check_image(image, 0x4000, EM_AARCH64), [])

    def test_accepts_x86_64_64k_load_segments(self):
        image = parse_elf(synthetic_elf(EM_X86_64, [(0, 0, 0x10000)]))
        self.assertEqual(check_image(image, 0x4000, EM_X86_64), [])

    def test_rejects_p_align_below_requested_page(self):
        image = parse_elf(synthetic_elf(EM_AARCH64, [(0, 0, 0x1000)]))
        errors = check_image(image, 0x4000)
        self.assertTrue(any("below page size" in error for error in errors))

    def test_rejects_page_offset_mismatch(self):
        image = parse_elf(synthetic_elf(EM_AARCH64, [(0x1000, 0x2000, 0x10000)]))
        errors = check_image(image, 0x4000)
        self.assertTrue(any("congruent modulo page size" in error for error in errors))

    def test_checks_elf_alignment_congruence_separately(self):
        image = parse_elf(synthetic_elf(EM_AARCH64, [(0, 0x4000, 0x8000)]))
        errors = check_image(image, 0x4000)
        self.assertFalse(any("congruent modulo page size" in error for error in errors))
        self.assertTrue(any("congruent modulo p_align" in error for error in errors))

    def test_can_enforce_expected_machine(self):
        image = parse_elf(synthetic_elf(EM_X86_64, [(0, 0, 0x10000)]))
        self.assertTrue(any("expected machine" in error for error in check_image(image, 0x4000, EM_AARCH64)))

    def test_parses_big_endian_elf64(self):
        image = parse_elf(synthetic_elf(EM_AARCH64, [(0, 0, 0x10000)], endian=">"))
        self.assertEqual(image.byte_order, "big")
        self.assertEqual(check_image(image, 0x4000), [])

    def test_rejects_non_elf_and_malformed_program_table(self):
        with self.assertRaises(ValueError):
            parse_elf(b"not an ELF")
        malformed = bytearray(synthetic_elf(EM_AARCH64, [(0, 0, 0x10000)]))
        struct.pack_into("<Q", malformed, 32, len(malformed) + 1)
        with self.assertRaisesRegex(ValueError, "extends beyond"):
            parse_elf(bytes(malformed))


if __name__ == "__main__":
    unittest.main()
