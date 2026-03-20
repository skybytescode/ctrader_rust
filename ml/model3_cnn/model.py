"""
Model 3 — 1D CNN Chart Pattern Classifier
==========================================
Architecture (per direction: long and short):

    Input : (batch, 5, WINDOW_SIZE)  — 5 channels: O, H, L, C, Volume
    ────────────────────────────────────────────────────────────────────
    Conv1d(5→32,  k=3, pad=1) → BatchNorm → ReLU → MaxPool(2)
    Conv1d(32→64, k=3, pad=1) → BatchNorm → ReLU → MaxPool(2)
    Conv1d(64→128,k=3, pad=1) → BatchNorm → ReLU → MaxPool(2)
    AdaptiveAvgPool1d(1)       — global average pooling → (batch, 128)
    ────────────────────────────────────────────────────────────────────
    Linear(128→64) → ReLU → Dropout(p)
    Linear(64→1)                       — raw logit (no activation)
    ────────────────────────────────────────────────────────────────────
    forward()      → logits   (use with BCEWithLogitsLoss in training)
    predict_proba()→ sigmoid(logits)   (use at inference time)

WINDOW_SIZE=60 → after 3 × MaxPool(2): 60 → 30 → 15 → 7,
AdaptiveAvgPool collapses to 1 regardless of temporal length.
"""

import torch
import torch.nn as nn


class CNNPatternModel(nn.Module):
    def __init__(
        self,
        window_size: int   = 60,
        n_channels:  int   = 5,
        dropout:     float = 0.3,
    ):
        super().__init__()
        self.window_size = window_size
        self.n_channels  = n_channels

        self.conv_blocks = nn.Sequential(
            # ── Block 1 ──────────────────────────────────────────────
            nn.Conv1d(n_channels, 32, kernel_size=3, padding=1),
            nn.BatchNorm1d(32),
            nn.ReLU(inplace=True),
            nn.MaxPool1d(kernel_size=2),

            # ── Block 2 ──────────────────────────────────────────────
            nn.Conv1d(32, 64, kernel_size=3, padding=1),
            nn.BatchNorm1d(64),
            nn.ReLU(inplace=True),
            nn.MaxPool1d(kernel_size=2),

            # ── Block 3 ──────────────────────────────────────────────
            nn.Conv1d(64, 128, kernel_size=3, padding=1),
            nn.BatchNorm1d(128),
            nn.ReLU(inplace=True),
            nn.MaxPool1d(kernel_size=2),
        )

        # Global average pooling → fixed-size representation regardless of window length
        self.global_pool = nn.AdaptiveAvgPool1d(output_size=1)

        self.head = nn.Sequential(
            nn.Linear(128, 64),
            nn.ReLU(inplace=True),
            nn.Dropout(p=dropout),
            nn.Linear(64, 1),
        )

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        """
        Parameters
        ----------
        x : (batch, n_channels, window_size)

        Returns
        -------
        logits : (batch,)  — raw scores, NOT probabilities.
            Use BCEWithLogitsLoss in training.
            Call predict_proba() for inference probabilities.
        """
        x = self.conv_blocks(x)          # (batch, 128, T')
        x = self.global_pool(x)          # (batch, 128, 1)
        x = x.squeeze(-1)               # (batch, 128)
        x = self.head(x)                # (batch, 1)
        return x.squeeze(-1)            # (batch,)

    def predict_proba(self, x: torch.Tensor) -> torch.Tensor:
        """Return win probabilities in [0, 1]."""
        return torch.sigmoid(self.forward(x))


def count_params(model: nn.Module) -> int:
    """Return total number of trainable parameters."""
    return sum(p.numel() for p in model.parameters() if p.requires_grad)
