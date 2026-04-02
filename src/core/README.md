# @simlin/core

Shared data models and utilities for the [Simlin](https://github.com/bpowers/simlin) system dynamics toolkit.

Provides protobuf-based data structures (`Project`, `Model`, `Variable`, `Equation`, `Dimension`), variable name canonicalization, collection utilities, and error type definitions.

## Install

```bash
npm install @simlin/core
```

**Peer dependency**: `@simlin/engine` must be installed alongside this package.

## Subpath Imports

```ts
import { Project, Variable, projectFromJson } from '@simlin/core/datamodel';
import { canonicalize } from '@simlin/core/canonicalize';
import { Series, defined, exists } from '@simlin/core/common';
import { first, last, getOrThrow } from '@simlin/core/collections';
import { ErrorCode, errorCodeDescription } from '@simlin/core/errors';
```

## License

Apache-2.0
