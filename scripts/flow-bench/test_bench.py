"""Offline tests for the contention retry in bench.py. Run: python3 scripts/flow-bench/test_bench.py"""

from __future__ import annotations

import unittest
from unittest import mock

import bench


def contention(retry_after_ms: int = 200, elapsed_ms: float = 2099.5) -> bench.ApiError:
    return bench.ApiError(
        "POST /api/v1/flow/objects/x/commands HTTP 200 in 2099.5 ms: contention",
        envelope={
            "code": 409,
            "message": "server_draining",
            "data": None,
            "error_code": "server_draining",
            "details": {"reason": "contention", "retry_after_ms": retry_after_ms},
        },
        elapsed_ms=elapsed_ms,
    )


def ok_result(elapsed: float = 10.0) -> tuple[dict, float, int]:
    return {"accepted_seq": 1}, elapsed, 10


def scripted(outcomes: list):
    """Build a send() that yields each outcome in turn: an exception is raised, a tuple returned."""
    calls = []

    def send():
        calls.append(1)
        outcome = outcomes[len(calls) - 1]
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    send.calls = calls
    return send


class RetryTests(unittest.TestCase):
    def test_success_first_time_has_no_retries(self) -> None:
        waits: list[float] = []
        send = scripted([ok_result(elapsed=12.5)])
        data, total_ms, retry_waits = bench.send_with_contention_retry(send, sleep=waits.append)
        self.assertEqual(data, {"accepted_seq": 1})
        self.assertEqual(total_ms, 12.5)
        self.assertEqual(retry_waits, [])
        self.assertEqual(waits, [])
        self.assertEqual(len(send.calls), 1)

    def test_contention_twice_then_success_reports_two_retries(self) -> None:
        waits: list[float] = []
        send = scripted([contention(200, 2000.0), contention(1000, 1500.0), ok_result(elapsed=30.0)])
        data, total_ms, retry_waits = bench.send_with_contention_retry(send, sleep=waits.append)
        self.assertEqual(data, {"accepted_seq": 1})
        self.assertEqual(len(send.calls), 3)
        self.assertEqual(retry_waits, [250.0, 1000.0])  # 200 ms raised to the 250 ms floor, 1000 honoured
        self.assertEqual(waits, [0.25, 1.0])
        self.assertEqual(total_ms, 2000.0 + 1500.0 + 30.0)  # every attempt counts toward the sample

    def test_contention_until_bound_fails_with_retries_reported(self) -> None:
        waits: list[float] = []
        send = scripted([contention(), contention(), contention(), contention()])
        with self.assertRaises(bench.ApiError) as ctx:
            bench.send_with_contention_retry(send, sleep=waits.append)
        err = ctx.exception
        self.assertIn("contention retries exhausted after 3 retries", str(err))
        self.assertEqual(len(err.retries), bench.CONTENTION_RETRY_LIMIT)
        self.assertEqual(len(send.calls), bench.CONTENTION_RETRY_LIMIT + 1)
        self.assertEqual(waits, [0.25, 0.25, 0.25])

    def test_non_contention_error_is_not_retried(self) -> None:
        waits: list[float] = []
        drain = bench.ApiError(
            "drain",
            envelope={"error_code": "server_draining", "details": {"reason": "drain", "retry_after_ms": 200}},
            elapsed_ms=5.0,
        )
        send = scripted([drain])
        with self.assertRaises(bench.ApiError) as ctx:
            bench.send_with_contention_retry(send, sleep=waits.append)
        self.assertIs(ctx.exception, drain)
        self.assertEqual(ctx.exception.retries, [])
        self.assertEqual(waits, [])
        self.assertEqual(len(send.calls), 1)

    def test_http_error_without_envelope_is_not_retried(self) -> None:
        plain = bench.ApiError("POST ... HTTP 500 in 3.0 ms: boom", elapsed_ms=3.0)
        send = scripted([plain])
        with self.assertRaises(bench.ApiError):
            bench.send_with_contention_retry(send, sleep=lambda _s: self.fail("must not sleep"))
        self.assertEqual(len(send.calls), 1)

    def test_error_after_a_retry_keeps_the_retry_count(self) -> None:
        plain = bench.ApiError("POST ... HTTP 500", elapsed_ms=4.0)
        send = scripted([contention(), plain])
        with self.assertRaises(bench.ApiError) as ctx:
            bench.send_with_contention_retry(send, sleep=lambda _s: None)
        self.assertEqual(len(ctx.exception.retries), 1)


class SemanticBatchesTests(unittest.TestCase):
    def fake_call(self, rejections_by_batch: dict[int, int]):
        """Reject the first N attempts of a batch with contention, then accept.

        Batches are told apart by the first operation's block id (b00000, b00050, ...).
        """
        seen: dict[str, int] = {}
        bodies: list[dict] = []

        def fake(method, url, token, payload, timeout):
            block = payload["command"]["payload"]["operations"][0]["id"]
            batch = int(block[1:]) // bench.BLOCKS_PER_BATCH
            bodies.append(payload)
            attempt = seen.get(block, 0)
            seen[block] = attempt + 1
            if attempt < rejections_by_batch.get(batch, 0):
                raise contention()
            return {"accepted_seq": batch}, 7.0, 10

        return fake, bodies

    def test_retries_are_counted_and_batch_still_completes(self) -> None:
        fake, bodies = self.fake_call({1: 2})
        with mock.patch.object(bench, "call", side_effect=fake), mock.patch("time.sleep"):
            result = bench.semantic_batches("http://x", "t", "obj", 150, "flowbench")
        self.assertTrue(result["ok"])
        self.assertEqual(result["batches_completed"], 3)
        self.assertEqual(result["retries_per_batch"], [0, 2, 0])
        self.assertEqual(result["retries_total"], 2)
        self.assertEqual(result["retry_limit_per_batch"], 3)
        self.assertEqual(result["failures"], [])
        # The retried batch resent the identical body, including its idempotency key.
        batch1 = [b for b in bodies if b["command"]["payload"]["operations"][0]["id"] == "b00050"]
        self.assertEqual(len(batch1), 3)
        self.assertTrue(all(b == batch1[0] for b in batch1))

    def test_exhausted_bound_fails_and_reports_retries(self) -> None:
        fake, _bodies = self.fake_call({1: 99})
        with mock.patch.object(bench, "call", side_effect=fake), mock.patch("time.sleep"):
            result = bench.semantic_batches("http://x", "t", "obj", 150, "flowbench")
        self.assertFalse(result["ok"])
        self.assertEqual(result["batches_completed"], 1)
        self.assertEqual(result["retries_per_batch"], [0])
        self.assertEqual(result["retries_total"], 3)
        self.assertEqual(len(result["failures"]), 1)
        self.assertIn("batch 1 at block 50 (after 3 contention retries)", result["failures"][0])
        self.assertIn("contention retries exhausted after 3 retries", result["failures"][0])


class RenderTests(unittest.TestCase):
    def test_fmt_retries_renders_new_fields(self) -> None:
        text = bench.fmt_retries(
            {"retries_total": 2, "retry_limit_per_batch": 3, "retries_per_batch": [0, 2, 0, 1]}
        )
        self.assertEqual(text, "total 2 (limit 3 per batch); 2 of 4 completed batches needed a retry")

    def test_fmt_retries_renders_old_data_without_fields(self) -> None:
        self.assertEqual(
            bench.fmt_retries({"ok": False, "batches_completed": 31}),
            "not recorded (run predates retry reporting)",
        )
        self.assertEqual(bench.fmt_retries(None), "not recorded (run predates retry reporting)")


if __name__ == "__main__":
    unittest.main()
