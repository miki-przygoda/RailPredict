"""
Deprecated entry point -- forwards to compare_models.py.

The original standalone trainer exported a 10/15-feature layout that no longer
matches the Rust inference code (14/22 features in onnx_engine.rs) and built
the real-time `current_delay_mins` feature as `delay_mins * U(0.7, 1.3)`, i.e.
the training label plus noise (target leakage).  It has been retired; this shim
keeps `python scripts/train_models.py` and old muscle memory working by running
the maintained, leak-free pipeline instead.

Usage:
    python train_models.py [compare_models.py args, e.g. --since 2026-07-01]
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from compare_models import main  # noqa: E402

if __name__ == "__main__":
    print("train_models.py is deprecated -- running compare_models.py\n", file=sys.stderr)
    main()
