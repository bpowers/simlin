// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

type Ptr = number;
type Finalizer = (ptr: Ptr) => void;
type FinalizerRecord = { ptr: Ptr; dispose: Finalizer };

interface FinalizationRegistryLike<T> {
  register(target: object, heldValue: T, unregisterToken?: object): void;
  unregister(unregisterToken: object): boolean;
}

interface FinalizationRegistryConstructorLike {
  new <T>(cleanup: (heldValue: T) => void): FinalizationRegistryLike<T>;
}

const FinalizationRegistryImpl = (globalThis as { FinalizationRegistry?: FinalizationRegistryConstructorLike })
  .FinalizationRegistry;

const registry: FinalizationRegistryLike<FinalizerRecord> | null = FinalizationRegistryImpl
  ? new FinalizationRegistryImpl<FinalizerRecord>((held) => {
      try {
        held.dispose(held.ptr);
      } catch {
        // Best-effort cleanup only.
      }
    })
  : null;

export function registerFinalizer(owner: object, ptr: Ptr, dispose: Finalizer): void {
  registry?.register(owner, { ptr, dispose }, owner);
}

export function unregisterFinalizer(owner: object): void {
  registry?.unregister(owner);
}
