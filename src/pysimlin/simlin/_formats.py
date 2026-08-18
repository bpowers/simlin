"""The single on-disk format table for pysimlin.

pattern: Functional Core (pure; no FFI, no I/O)

Every place that maps a path or a byte payload to a model format --
``simlin.load()``, ``simlin.open()``, ``Project.save()``,
``Project.save_as()`` -- resolves it through this module.  Divergent copies
are how a file becomes loadable but unsavable (or the reverse), so new
consumers must call these functions rather than re-deriving the mapping.

The suffix table mirrors ``simlin-serve``'s ``discovery::format_for_path``
(``.stmx``/``.xmile``/``.xml`` -> XMILE, ``.mdl`` -> Vensim, ``.sd.json`` ->
native JSON) and adds what pysimlin's own readers already accepted
(``.vpm``, protobuf suffixes, plain ``.json``).  JSON payloads are sniffed on
their top-level key exactly as ``simlin-mcp-core::open::open_project`` does:
``models`` is native Simlin JSON, ``variables`` is SD-AI JSON.  ``.sd.json``
is the *write* default for native JSON; on read it is sniffed like any other
JSON suffix, because SD-AI payloads exist under that suffix too
(``test/sd-ai-simple.sd.json``).
"""

from __future__ import annotations

import enum
import json
from pathlib import Path
from typing import Union

from .errors import SimlinImportError

_PathLike = Union[str, Path]


class FileFormat(enum.Enum):
    """An on-disk model format pysimlin can read and write.

    ``XMILE`` and ``MDL`` are regenerated (not byte-preserved) on save, as
    everywhere else in the repo; ``NATIVE_JSON`` is Simlin's own JSON
    (pretty-printed for git-friendly diffs); ``SDAI_JSON`` is the SD-AI
    interchange format; ``PROTOBUF`` is the binary datamodel encoding.
    """

    XMILE = "xmile"
    MDL = "mdl"
    NATIVE_JSON = "json"
    SDAI_JSON = "sd-ai"
    PROTOBUF = "protobuf"


_XMILE_SUFFIXES = frozenset({".stmx", ".xmile", ".xml"})
_MDL_SUFFIXES = frozenset({".mdl", ".vpm"})
_JSON_SUFFIXES = frozenset({".json"})
_PROTOBUF_SUFFIXES = frozenset({".pb", ".bin", ".proto"})


def format_for_suffix(path: _PathLike) -> FileFormat | None:
    """Map a path to a format purely by its (case-insensitive) suffix.

    ``.json`` and ``.sd.json`` both map to ``NATIVE_JSON``: this is the
    format a *writer* should use for that suffix.  Readers must call
    :func:`resolve_read_format`, which sniffs JSON payloads for SD-AI.
    Returns ``None`` for suffixes the table does not know.
    """
    suffix = Path(path).suffix.lower()
    if suffix in _XMILE_SUFFIXES:
        return FileFormat.XMILE
    if suffix in _MDL_SUFFIXES:
        return FileFormat.MDL
    if suffix in _JSON_SUFFIXES:
        return FileFormat.NATIVE_JSON
    if suffix in _PROTOBUF_SUFFIXES:
        return FileFormat.PROTOBUF
    return None


def _strip_leading(data: bytes) -> bytes:
    """Drop a UTF-8 BOM and leading whitespace so the sniff sees content."""
    if data.startswith(b"\xef\xbb\xbf"):
        data = data[3:]
    return data.lstrip()


def _sniff_json(data: bytes) -> FileFormat | None:
    """Classify a JSON payload by its top-level key.

    Returns ``NATIVE_JSON`` for a ``models`` key, ``SDAI_JSON`` for a
    ``variables`` key, and ``None`` when the bytes are not a JSON object
    or carry neither key.
    """
    try:
        value = json.loads(data)
    except (ValueError, UnicodeDecodeError):
        return None
    if not isinstance(value, dict):
        return None
    if "models" in value:
        return FileFormat.NATIVE_JSON
    if "variables" in value:
        return FileFormat.SDAI_JSON
    return None


def _looks_like_vensim(data: bytes) -> bool:
    """Heuristic for a Vensim ``.mdl`` body without the ``{UTF-8}`` header.

    Vensim equations are ``name = expr ~ units ~ doc |`` records, and any
    model with a diagram carries the sketch marker; either is enough to
    prefer the Vensim reader over "unrecognisable".
    """
    return b"\\---///" in data or (b"~" in data and b"|" in data)


def sniff_format(data: bytes) -> FileFormat | None:
    """Guess a format from a byte payload alone.

    Used when the suffix says nothing.  Protobuf has no magic bytes and is
    deliberately not guessed: a binary blob is "unrecognisable" unless the
    suffix names it.  Returns ``None`` when nothing matches.
    """
    body = _strip_leading(data)
    if not body:
        return None
    if body.startswith(b"{UTF-8}"):
        return FileFormat.MDL
    if body.startswith(b"<"):
        return FileFormat.XMILE
    if body.startswith((b"{", b"[")):
        return _sniff_json(body)
    if _looks_like_vensim(body):
        return FileFormat.MDL
    return None


def resolve_read_format(path: _PathLike, data: bytes) -> FileFormat:
    """Decide which reader to hand ``data`` (the contents of ``path``) to.

    An unambiguous suffix (XMILE, Vensim, protobuf) wins outright, so a
    malformed ``.stmx`` reaches the XMILE reader and the engine reports the
    parse error.  JSON suffixes sniff the payload for native vs SD-AI;
    invalid JSON under a JSON suffix resolves to native JSON for the same
    reason (the engine's error is the useful one).  An unknown suffix falls
    back to :func:`sniff_format`.

    Raises:
        SimlinImportError: naming ``path`` when neither suffix nor content
            identifies a format, or when a JSON payload has neither a
            ``models`` nor a ``variables`` key.
    """
    p = Path(path)
    by_suffix = format_for_suffix(p)
    if by_suffix is FileFormat.NATIVE_JSON:
        sniffed = _sniff_json(_strip_leading(data))
        if sniffed is not None:
            return sniffed
        try:
            json.loads(data)
        except (ValueError, UnicodeDecodeError):
            return FileFormat.NATIVE_JSON
        raise SimlinImportError(
            f"unrecognized JSON format in {p}: expected a top-level 'models' key "
            f"(Simlin JSON) or 'variables' key (SD-AI JSON)"
        )
    if by_suffix is not None:
        return by_suffix
    sniffed = sniff_format(data)
    if sniffed is None:
        raise SimlinImportError(
            f"cannot determine the model format of {p}: unrecognized suffix "
            f"{p.suffix!r} and the contents are not XMILE, Vensim, or JSON"
        )
    return sniffed


def resolve_write_format(path: _PathLike, format: FileFormat | None) -> FileFormat:
    """Decide the format to write ``path`` in.

    An explicit ``format`` always wins (so a model can be saved as XMILE
    under any name); otherwise the suffix decides.

    Raises:
        ValueError: when the suffix is unknown and no format was given.
    """
    if format is not None:
        return format
    by_suffix = format_for_suffix(path)
    if by_suffix is None:
        raise ValueError(
            f"cannot infer a model format from the suffix of {Path(path)}; "
            f"pass format= explicitly (one of {', '.join(f.name for f in FileFormat)})"
        )
    return by_suffix


__all__ = [
    "FileFormat",
    "format_for_suffix",
    "resolve_read_format",
    "resolve_write_format",
    "sniff_format",
]
