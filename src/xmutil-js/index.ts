// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Re-export from the platform-specific implementation
// The path mapping in tsconfig resolves this to either index.node.ts or index.browser.ts
export * from '@system-dynamics/xmutil/impl';
