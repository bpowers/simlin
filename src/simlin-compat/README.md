# simlin-compat

This crate provides compatibility functionality for converting between Simlin's internal project representation and external formats like XMILE and Vensim MDL.

## MDL Parser

The `mdl` module contains a native Rust parser for Vensim MDL files. This parser is a derivative work based on Bob Eberlein's [xmutil](https://github.com/bobeberlein/xmutil), which converts Vensim models to XMILE format.

The original xmutil is MIT licensed, and this derivative work maintains that license.

## License

MIT License - see [LICENSE](LICENSE) for details.
