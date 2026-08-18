"""Read-only diagram rendering for notebooks.

pattern: Functional Core (a value object; rendering happens in the engine)
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Diagram:
    """A rendered stock-and-flow diagram.

    Returned by :meth:`simlin.Model.diagram`.  Displaying it in a Jupyter,
    VS Code, or Colab cell shows the picture (via ``_repr_svg_``); ``svg``
    is the markup itself, ready to write to a ``.svg`` file.  A model with
    no diagram is drawn with a transient automatic layout.
    """

    svg: str

    def _repr_svg_(self) -> str:
        return self.svg

    def _repr_mimebundle_(self, include: object = None, exclude: object = None) -> dict[str, str]:
        return {"image/svg+xml": self.svg}


__all__ = ["Diagram"]
