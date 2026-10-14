"""Walk-forward splits must keep the test year out of training, including
the labels: a label looks `horizon_bars` ahead, so the last training rows
are purged.

Run from the repository root:  python -m pytest ml/tests
"""

import sys
import types

import numpy as np
import pandas as pd
import pytest

# The feature modules import pandas_ta at the top; the split logic does not
# need it, and it does not install on current numpy.
sys.modules.setdefault("pandas_ta", types.ModuleType("pandas_ta"))

from ml.model1_technical import train as model1  # noqa: E402
from ml.model4_econcal import train as model4  # noqa: E402

HORIZON = 120


def minute_bars(start: str, end: str) -> pd.DataFrame:
    index = pd.date_range(start, end, freq="1min", inclusive="left")
    return pd.DataFrame({"close": np.arange(len(index), dtype=float)}, index=index)


@pytest.mark.parametrize("split", [model1.walk_forward_splits, model4.walk_forward_splits])
def test_folds_are_chronological_and_purged(split):
    data = minute_bars("2021-12-31", "2024-01-02")  # 2021 (1 day), 2022, 2023, 2024 (1 day)
    folds = split(data, n_folds=2, embargo=HORIZON)
    assert [year for _, _, year in folds] == [2023, 2024]

    for train_idx, test_idx, year in folds:
        assert (data.index[test_idx].year == year).all()
        assert (data.index[train_idx].year < year).all()
        first_test = test_idx[0]
        # The gap between the last training row and the first test row covers
        # the label horizon.
        assert first_test - train_idx[-1] > HORIZON


def test_without_embargo_training_runs_up_to_the_test_year():
    data = minute_bars("2022-01-01", "2024-01-01")
    (train_idx, test_idx, _), = model1.walk_forward_splits(data, n_folds=1)
    assert test_idx[0] - train_idx[-1] == 1


def test_needs_more_years_than_folds():
    with pytest.raises(ValueError):
        model1.walk_forward_splits(minute_bars("2023-01-01", "2023-02-01"), n_folds=1)
