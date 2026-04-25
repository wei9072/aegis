"""
Delivery layer — formats policy verdicts into a human-visible view.

Phase 1 contract:
  - Warning banner appears BEFORE the code block.
  - Banner is human-only; the LLM-bound view is clean code with no
    warning text. This satisfies the "delivery isolation" invariant —
    the next LLM turn never re-ingests the warning as context.
  - The renderer makes no decisions; it only formats whatever the
    policy engine produced.
"""
from aegis.delivery.renderer import DeliveryRenderer, DeliveryView

__all__ = ["DeliveryRenderer", "DeliveryView"]
