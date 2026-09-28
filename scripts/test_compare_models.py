"""
Leakage regression tests for compare_models.py -- no database needed.

    python -m unittest scripts/test_compare_models.py

Needs pandas + numpy (scripts/requirements.txt).  The heavy training imports
(lightgbm, onnxmltools, sqlalchemy, dotenv) are stubbed if absent.
"""

import sys
import types
import unittest
from pathlib import Path

import numpy as np
import pandas as pd

sys.path.insert(0, str(Path(__file__).parent))

# Stub optional heavy deps so the feature code can be tested in isolation.
for name, attrs in {
    "lightgbm": ["LGBMRegressor", "early_stopping", "log_evaluation"],
    "onnxmltools": ["convert_lightgbm"],
    "onnxmltools.convert": [],
    "onnxmltools.convert.common": [],
    "onnxmltools.convert.common.data_types": ["FloatTensorType"],
    "sqlalchemy": ["create_engine", "text"],
    "dotenv": ["load_dotenv"],
    "sklearn": [],
    "sklearn.metrics": ["mean_absolute_error", "root_mean_squared_error"],
}.items():
    try:
        __import__(name)
    except ImportError:
        mod = types.ModuleType(name)
        for a in attrs:
            setattr(mod, a, lambda *a, **k: None)
        sys.modules[name] = mod

import compare_models as cm  # noqa: E402


def make_frame() -> pd.DataFrame:
    """Three weekly runs of one pattern, each with three snapshots, plus a
    second pattern at the same station so the cross-train features have data."""
    rows = []
    for week in range(3):
        day = pd.Timestamp("2026-06-08", tz="UTC") + pd.Timedelta(days=7 * week)
        for k, delay in enumerate([2 + week, 5 + week, 9 + week]):
            rows.append(("C10001", 0, "LDS", 9, delay, day + pd.Timedelta(hours=8, minutes=10 * k)))
            rows.append(("G20002", 0, "LDS", 9, 1, day + pd.Timedelta(hours=8, minutes=10 * k + 3)))
    return pd.DataFrame(rows, columns=["uid", "weekday", "origin_crs", "departure_hour",
                                       "delay_mins", "recorded_at"])


class LeakageTests(unittest.TestCase):
    def setUp(self):
        df = cm.add_journey_columns(make_frame())
        df = cm.add_rolling_features(df)
        self.df = cm.engineer_features(df)

    def test_realtime_target_is_the_journey_final_reading(self):
        j = self.df[self.df["uid"] == "C10001"].sort_values("recorded_at").head(3)
        self.assertEqual(list(j["final_delay_mins"]), [9.0, 9.0, 9.0])
        self.assertEqual(list(j["is_journey_final"]), [False, False, True])

    def test_current_delay_is_the_snapshot_not_the_target(self):
        rt = self.df[~self.df["is_journey_final"] & (self.df["uid"] == "C10001")]
        # Features are the earlier readings; the target is strictly later.
        self.assertTrue((rt["current_delay_mins"] == rt["delay_mins"]).all())
        self.assertTrue((rt["current_delay_mins"] != rt["final_delay_mins"]).all())

    def test_rolling_features_exclude_the_current_run(self):
        c = self.df[self.df["uid"] == "C10001"].sort_values("recorded_at")
        first_week = c.head(3)
        self.assertTrue((first_week["rolling_mean_7d"] == 0).all(),
                        "first run has no prior runs, so no rolling history")
        second_week = c.iloc[3:6]
        # Prior run read 2, 5, 9 (at 08:00/08:10/08:20).  The 7-day window slides
        # with each snapshot, but it must never include this run's own readings
        # (3, 6 at 08:00/08:10), which would pull the later means down.
        expected = [16 / 3, 7.0, 9.0]
        for v, e in zip(second_week["rolling_mean_7d"], expected):
            self.assertAlmostEqual(v, e, places=4)

    def test_no_realtime_feature_reads_the_final_delay(self):
        for col in cm.FEATURE_COLS_RT:
            if col in self.df:
                self.assertFalse(
                    np.allclose(self.df[col].values, self.df["final_delay_mins"].values),
                    f"{col} reproduces the real-time target",
                )

    def test_temporal_split_is_date_ordered_and_disjoint(self):
        early, late = cm.temporal_split(self.df, 0.34)
        self.assertLess(early["service_date"].max(), late["service_date"].min())
        keys = cm.JOURNEY_KEYS
        shared = early[keys].drop_duplicates().merge(late[keys].drop_duplicates())
        self.assertTrue(shared.empty, "a journey straddles the split")

    def test_encodings_fitted_on_train_only(self):
        early, late = cm.temporal_split(self.df, 0.34)
        late = late.copy()
        late.loc[late.index[0], "origin_crs"] = "ZZZ"
        meta = cm.fit_encodings(early)
        self.assertNotIn("ZZZ", meta["crs"])
        enc = cm.apply_encodings(late, meta)
        self.assertEqual(enc.loc[late.index[0], "origin_crs_enc"], 0)


if __name__ == "__main__":
    unittest.main()
