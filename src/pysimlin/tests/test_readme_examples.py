"""Test that README.md code examples execute correctly.

This test extracts Python code blocks from README.md and executes them
sequentially in a shared namespace, ensuring documentation stays in sync
with the actual API.

Directives (HTML comments immediately before code blocks):
- <!-- pysimlin-test: skip --> - Don't execute this block
- <!-- pysimlin-test: expect-error --> - Block should raise an exception
- <!-- pysimlin-test: reset --> - Clear namespace before this block

SECURITY NOTE: This module uses exec() to run code extracted from README.md.
This is acceptable because README.md is version-controlled and changes require
PR review. Do not copy this pattern for use with untrusted input sources.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import NamedTuple

import pytest


class CodeBlock(NamedTuple):
    """A Python code block extracted from markdown."""

    directive: str  # empty string if no directive
    code: str
    line_number: int  # 1-indexed line where ```python appears


def extract_python_blocks(markdown: str) -> list[CodeBlock]:
    """Extract Python code blocks with their preceding directives.

    Scans the markdown line by line, looking for:
    1. Optional HTML comment directive: <!-- pysimlin-test: DIRECTIVE -->
    2. Followed by ```python code block

    Returns list of CodeBlock tuples with (directive, code, line_number).
    """
    blocks: list[CodeBlock] = []
    lines = markdown.split("\n")

    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()

        # Check for directive comment
        directive = ""
        directive_match = re.match(r"<!--\s*pysimlin-test:\s*(\w+(?:-\w+)*)\s*-->", stripped)
        if directive_match:
            directive = directive_match.group(1)
            i += 1
            # Skip any blank lines between directive and code block
            while i < len(lines) and not lines[i].strip():
                i += 1
            if i >= len(lines):
                break
            stripped = lines[i].strip()

        # Check for python code block start
        if stripped == "```python":
            block_start_line = i + 1  # 1-indexed for display
            i += 1
            code_lines: list[str] = []

            # Collect lines until closing ```
            while i < len(lines):
                if lines[i].strip() == "```":
                    break
                code_lines.append(lines[i])
                i += 1

            code = "\n".join(code_lines)
            blocks.append(CodeBlock(directive=directive, code=code, line_number=block_start_line))

        i += 1

    return blocks


class TestReadmeExamples:
    """Execute README.md code examples to verify they work."""

    @pytest.fixture
    def readme_path(self) -> Path:
        """Path to the README.md file."""
        return Path(__file__).parent.parent / "README.md"

    @pytest.fixture
    def code_blocks(self, readme_path: Path) -> list[CodeBlock]:
        """Extract all Python code blocks from README."""
        if not readme_path.exists():
            pytest.fail(f"README.md not found at {readme_path}")

        markdown = readme_path.read_text()
        blocks = extract_python_blocks(markdown)

        if not blocks:
            pytest.fail("No Python code blocks found in README.md")

        return blocks

    def test_all_python_blocks_execute(self, code_blocks: list[CodeBlock]) -> None:
        """All non-skipped Python blocks should execute without error."""
        # Shared namespace for sequential execution
        # Set __name__ so `if __name__ == "__main__"` guards pass
        namespace: dict[str, object] = {"__name__": "__main__"}

        for block in code_blocks:
            directive = block.directive
            code = block.code
            line_num = block.line_number

            if directive == "skip":
                continue

            if directive == "reset":
                namespace = {"__name__": "__main__"}

            expect_error = directive == "expect-error"

            # Create a meaningful test context for error messages
            code_preview = code[:300] + "..." if len(code) > 300 else code

            try:
                # SECURITY: exec() is safe here because README.md is version-controlled
                # and undergoes PR review. DO NOT use this pattern with untrusted input.
                exec(code, namespace)

                if expect_error:
                    pytest.fail(
                        f"README.md line {line_num}: Block was expected to raise "
                        f"an error but succeeded.\n\nCode:\n{code_preview}"
                    )

            except SyntaxError as e:
                # Syntax errors are always failures (even with expect-error,
                # we want valid Python that raises at runtime)
                pytest.fail(
                    f"README.md line {line_num}: Syntax error: {e}\n\nCode:\n{code_preview}"
                )

            except Exception as e:
                if not expect_error:
                    pytest.fail(
                        f"README.md line {line_num}: "
                        f"{type(e).__name__}: {e}\n\n"
                        f"Code:\n{code_preview}"
                    )


class TestCodeBlockExtraction:
    """Unit tests for the code block extraction logic."""

    def test_extracts_simple_block(self) -> None:
        """Extract a simple Python code block."""
        markdown = """
Some text

```python
x = 1
print(x)
```

More text
"""
        blocks = extract_python_blocks(markdown)
        assert len(blocks) == 1
        assert blocks[0].code == "x = 1\nprint(x)"
        assert blocks[0].directive == ""

    def test_extracts_block_with_directive(self) -> None:
        """Extract a block with a preceding directive."""
        markdown = """
<!-- pysimlin-test: skip -->
```python
import something_unavailable
```
"""
        blocks = extract_python_blocks(markdown)
        assert len(blocks) == 1
        assert blocks[0].directive == "skip"
        assert "import something_unavailable" in blocks[0].code

    def test_extracts_multiple_blocks(self) -> None:
        """Extract multiple blocks, some with directives."""
        markdown = """
```python
a = 1
```

<!-- pysimlin-test: reset -->
```python
b = 2
```

<!-- pysimlin-test: expect-error -->
```python
raise ValueError("expected")
```
"""
        blocks = extract_python_blocks(markdown)
        assert len(blocks) == 3
        assert blocks[0].directive == ""
        assert blocks[1].directive == "reset"
        assert blocks[2].directive == "expect-error"

    def test_ignores_non_python_blocks(self) -> None:
        """Only extract Python blocks, ignore bash/other."""
        markdown = """
```bash
pip install something
```

```python
x = 1
```

```javascript
const y = 2;
```
"""
        blocks = extract_python_blocks(markdown)
        assert len(blocks) == 1
        assert "x = 1" in blocks[0].code

    def test_directive_with_blank_lines(self) -> None:
        """Directive followed by blank lines before code block."""
        markdown = """
<!-- pysimlin-test: skip -->

```python
skipped_code()
```
"""
        blocks = extract_python_blocks(markdown)
        assert len(blocks) == 1
        assert blocks[0].directive == "skip"

    def test_preserves_indentation(self) -> None:
        """Code block indentation should be preserved."""
        markdown = """
```python
def foo():
    if True:
        return 42
```
"""
        blocks = extract_python_blocks(markdown)
        assert len(blocks) == 1
        assert "    if True:" in blocks[0].code
        assert "        return 42" in blocks[0].code

    def test_line_numbers_are_correct(self) -> None:
        """Line numbers should point to the ```python line."""
        markdown = """line 1
line 2
```python
code here
```
line 6
"""
        blocks = extract_python_blocks(markdown)
        assert len(blocks) == 1
        # ```python is on line 3 (1-indexed)
        assert blocks[0].line_number == 3
