"""Tests for SVG and PNG rendering."""

import struct

import pytest

import simlin


class TestRenderSvg:
    """Test SVG rendering from projects."""

    def test_render_svg_returns_bytes(self, xmile_model_path) -> None:
        """render_svg should return non-empty bytes."""
        model = simlin.load(xmile_model_path)
        svg = model.project.render_svg()
        assert isinstance(svg, bytes)
        assert len(svg) > 0

    def test_render_svg_is_valid_svg(self, xmile_model_path) -> None:
        """render_svg output should contain SVG root element."""
        model = simlin.load(xmile_model_path)
        svg = model.project.render_svg()
        assert b"<svg" in svg

    def test_render_svg_string(self, xmile_model_path) -> None:
        """render_svg_string should return a valid SVG string."""
        model = simlin.load(xmile_model_path)
        svg_str = model.project.render_svg_string()
        assert isinstance(svg_str, str)
        assert "<svg" in svg_str

    def test_render_svg_explicit_model_name(self, xmile_model_path) -> None:
        """render_svg should accept an explicit model name."""
        model = simlin.load(xmile_model_path)
        names = model.project.get_model_names()
        svg = model.project.render_svg(names[0])
        assert b"<svg" in svg

    def test_render_svg_nonexistent_model_raises(self, xmile_model_path) -> None:
        """render_svg should raise for a nonexistent model name."""
        model = simlin.load(xmile_model_path)
        with pytest.raises(Exception):
            model.project.render_svg("nonexistent_model_xyz")


class TestRenderPng:
    """Test PNG rendering from projects."""

    PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

    def test_render_png_returns_bytes(self, xmile_model_path) -> None:
        """render_png should return non-empty bytes."""
        model = simlin.load(xmile_model_path)
        png = model.project.render_png()
        assert isinstance(png, bytes)
        assert len(png) > 8

    def test_render_png_has_valid_signature(self, xmile_model_path) -> None:
        """render_png output should start with the PNG file signature."""
        model = simlin.load(xmile_model_path)
        png = model.project.render_png()
        assert png[:8] == self.PNG_SIGNATURE

    def test_render_png_with_width(self, xmile_model_path) -> None:
        """render_png with explicit width should produce a valid PNG."""
        model = simlin.load(xmile_model_path)
        png = model.project.render_png(width=400)
        assert png[:8] == self.PNG_SIGNATURE

        # Parse IHDR chunk to verify width
        width = struct.unpack(">I", png[16:20])[0]
        assert width == 400

    def test_render_png_with_height(self, xmile_model_path) -> None:
        """render_png with explicit height should produce a valid PNG."""
        model = simlin.load(xmile_model_path)
        png = model.project.render_png(height=300)
        assert png[:8] == self.PNG_SIGNATURE

        # Parse IHDR chunk to verify height
        height = struct.unpack(">I", png[20:24])[0]
        assert height == 300

    def test_render_png_preserves_aspect_ratio(self, xmile_model_path) -> None:
        """Width-only and intrinsic renders should have the same aspect ratio."""
        model = simlin.load(xmile_model_path)

        intrinsic = model.project.render_png()
        scaled = model.project.render_png(width=800)

        iw = struct.unpack(">I", intrinsic[16:20])[0]
        ih = struct.unpack(">I", intrinsic[20:24])[0]
        sw = struct.unpack(">I", scaled[16:20])[0]
        sh = struct.unpack(">I", scaled[20:24])[0]

        intrinsic_ratio = iw / ih
        scaled_ratio = sw / sh
        assert abs(intrinsic_ratio - scaled_ratio) < 0.05

    def test_render_png_nonexistent_model_raises(self, xmile_model_path) -> None:
        """render_png should raise for a nonexistent model name."""
        model = simlin.load(xmile_model_path)
        with pytest.raises(Exception):
            model.project.render_png("nonexistent_model_xyz")

    def test_render_png_explicit_model_name(self, xmile_model_path) -> None:
        """render_png should accept an explicit model name."""
        model = simlin.load(xmile_model_path)
        names = model.project.get_model_names()
        png = model.project.render_png(names[0])
        assert png[:8] == self.PNG_SIGNATURE
