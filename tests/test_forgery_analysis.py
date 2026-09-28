import unittest

from daemon.forgery_analysis.analyzer import (
    AudioStreamAnalyzer,
    HashChainAnalyzer,
    InputBehaviorAnalyzer,
)


class AudioStreamAnalyzerTests(unittest.TestCase):
    def test_regular_rms_flagged(self):
        analyzer = AudioStreamAnalyzer(min_windows=10)
        for i in range(50):
            analyzer.ingest_buffer_hash({"rms_level": 0.5, "zero_crossing_rate": 0.3, "spectral_centroid_hz": 2000})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("too_regular_rms", names)

    def test_varied_rms_not_flagged(self):
        analyzer = AudioStreamAnalyzer(min_windows=10)
        import random
        rng = random.Random(42)
        for _ in range(50):
            analyzer.ingest_buffer_hash({"rms_level": rng.uniform(0.01, 0.9), "zero_crossing_rate": rng.uniform(0, 1)})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertNotIn("too_regular_rms", names)

    def test_metronomic_transitions_flagged(self):
        analyzer = AudioStreamAnalyzer(min_windows=5)
        for i in range(20):
            analyzer.ingest_transition({"timestamp_ms": i * 1000})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("metronomic_transitions", names)

    def test_superhuman_speed_flagged(self):
        analyzer = AudioStreamAnalyzer(min_windows=5)
        for i in range(20):
            analyzer.ingest_transition({"timestamp_ms": i * 50})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("superhuman_speed", names)

    def test_insufficient_data_returns_empty(self):
        analyzer = AudioStreamAnalyzer(min_windows=100)
        for i in range(10):
            analyzer.ingest_buffer_hash({"rms_level": 0.5})
        report = analyzer.analyze()
        self.assertEqual(len(report.flags), 0)


class InputBehaviorAnalyzerTests(unittest.TestCase):
    def test_regular_iki_flagged(self):
        analyzer = InputBehaviorAnalyzer(min_samples=10)
        for _ in range(50):
            analyzer.ingest_keystroke_batch({"mean_iki_ms": 100.0, "count": 1})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("too_regular_iki", names)

    def test_missing_pauses_flagged(self):
        analyzer = InputBehaviorAnalyzer(min_samples=10)
        for _ in range(100):
            analyzer.ingest_keystroke_batch({"mean_iki_ms": 50.0, "count": 1})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("missing_human_pauses", names)

    def test_superhuman_input_flagged(self):
        analyzer = InputBehaviorAnalyzer(min_samples=10)
        for _ in range(100):
            analyzer.ingest_keystroke_batch({"mean_iki_ms": 10.0, "count": 1})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("superhuman_input_speed", names)

    def test_suspicion_score_range(self):
        analyzer = InputBehaviorAnalyzer(min_samples=10)
        for _ in range(100):
            analyzer.ingest_keystroke_batch({"mean_iki_ms": 10.0, "count": 1})
        report = analyzer.analyze()
        self.assertGreaterEqual(report.suspicion_score, 0.0)
        self.assertLessEqual(report.suspicion_score, 1.0)


class HashChainAnalyzerTests(unittest.TestCase):
    def test_valid_chain_no_flags(self):
        analyzer = HashChainAnalyzer()
        analyzer.ingest_buffer_hash({"window_hash": "aaa", "prev_hash": "genesis", "timestamp_ms": 100})
        analyzer.ingest_buffer_hash({"window_hash": "bbb", "prev_hash": "aaa", "timestamp_ms": 200})
        analyzer.ingest_buffer_hash({"window_hash": "ccc", "prev_hash": "bbb", "timestamp_ms": 300})
        report = analyzer.analyze()
        self.assertEqual(len(report.flags), 0)

    def test_chain_break_flagged(self):
        analyzer = HashChainAnalyzer()
        analyzer.ingest_buffer_hash({"window_hash": "aaa", "prev_hash": "genesis", "timestamp_ms": 100})
        analyzer.ingest_buffer_hash({"window_hash": "bbb", "prev_hash": "WRONG", "timestamp_ms": 200})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("chain_break", names)

    def test_duplicate_hashes_flagged(self):
        analyzer = HashChainAnalyzer()
        analyzer.ingest_buffer_hash({"window_hash": "aaa", "prev_hash": "genesis", "timestamp_ms": 100})
        analyzer.ingest_buffer_hash({"window_hash": "aaa", "prev_hash": "aaa", "timestamp_ms": 200})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("duplicate_hashes", names)

    def test_timestamp_reversal_flagged(self):
        analyzer = HashChainAnalyzer()
        analyzer.ingest_buffer_hash({"window_hash": "a", "prev_hash": "genesis", "timestamp_ms": 200})
        analyzer.ingest_buffer_hash({"window_hash": "b", "prev_hash": "a", "timestamp_ms": 100})
        report = analyzer.analyze()
        names = [f.name for f in report.flags]
        self.assertIn("timestamp_reversal", names)

    def test_chain_break_severity_is_max(self):
        analyzer = HashChainAnalyzer()
        analyzer.ingest_buffer_hash({"window_hash": "a", "prev_hash": "genesis", "timestamp_ms": 100})
        analyzer.ingest_buffer_hash({"window_hash": "b", "prev_hash": "WRONG", "timestamp_ms": 200})
        report = analyzer.analyze()
        chain_flag = next(f for f in report.flags if f.name == "chain_break")
        self.assertEqual(chain_flag.severity, 1.0)


if __name__ == "__main__":
    unittest.main()


class FiniteNumberTests(unittest.TestCase):
    def test_integers_wider_than_a_float_are_rejected_not_raised(self):
        from daemon.forgery_analysis.analyzer import _finite_number

        self.assertIsNone(_finite_number(10**400))
        self.assertEqual(_finite_number(3), 3.0)
        self.assertIsNone(_finite_number(float("inf")))
