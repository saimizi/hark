"""Run after `cargo build`: python3 tests/terminal_delivery.py."""

import os
import pathlib
import pty
import select
import subprocess
import unittest


class TerminalDelivery(unittest.TestCase):
    def test_unregistered_terminals_receive_immediate_and_background_delivery(self):
        binary = pathlib.Path(__file__).resolve().parents[1] / "target/debug/hark"
        terminals = [pty.openpty(), pty.openpty()]
        try:
            names = [os.ttyname(slave) for _, slave in terminals]
            who = subprocess.check_output(["who"], text=True)
            for name in names:
                self.assertNotIn(name.removeprefix("/dev/"), who)

            recipient = subprocess.check_output(["id", "-un"], text=True).strip()
            # `deliver` is also the entry point used by systemd timers; neither
            # invocation has a controlling terminal or inherits these PTYs.
            for args in [[], ["deliver", "--to", recipient]]:
                with self.subTest(role=args):
                    message = "hark-pty-test-" + str(len(args))
                    result = subprocess.run(
                        [str(binary), *args, "-m", message],
                        capture_output=True, text=True, timeout=5,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertNotIn("write(1) failed", result.stderr)
                    for (master, _), name in zip(terminals, names):
                        self.assertIn("delivered to " + name, result.stderr)
                        self.assertTrue(select.select([master], [], [], 2)[0])
                        self.assertIn(message.encode(), os.read(master, 4096))
        finally:
            for pair in terminals:
                for fd in pair:
                    os.close(fd)


if __name__ == "__main__":
    unittest.main()
