// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

/// Provides stable UID allocation and tracking for model and view elements.
///
/// Named elements (non-empty ident) always receive the same UID on repeated
/// `alloc` calls.  Unnamed elements (empty ident) receive a fresh, unique UID
/// each time.  UIDs start at 1 so that 0 can serve as a sentinel for
/// uninitialized values.
pub struct UidManager {
    seen: HashMap<i32, String>,
    reverse: HashMap<String, i32>,
    next: i32,
}

impl UidManager {
    pub fn new() -> Self {
        Self {
            seen: HashMap::new(),
            reverse: HashMap::new(),
            next: 1,
        }
    }

    /// Allocate a UID.  Named elements (non-empty ident) always get the same
    /// UID on repeated calls.  Unnamed elements get unique monotonically
    /// increasing UIDs.
    pub fn alloc(&mut self, ident: &str) -> i32 {
        if !ident.is_empty()
            && let Some(&uid) = self.reverse.get(ident)
        {
            return uid;
        }

        let uid = self.next;
        self.next += 1;

        self.seen.insert(uid, ident.to_string());
        if !ident.is_empty() {
            self.reverse.insert(ident.to_string(), uid);
        }

        uid
    }

    /// Seed the manager with an existing UID-to-ident mapping.  Typically used
    /// when loading models that already have UIDs assigned.  Advances `next`
    /// past the registered UID to prevent collisions.  UID 0 is silently
    /// ignored.
    pub fn add(&mut self, uid: i32, ident: &str) {
        if uid == 0 {
            return;
        }

        self.seen.insert(uid, ident.to_string());
        if !ident.is_empty() {
            self.reverse.insert(ident.to_string(), uid);
        }

        if uid >= self.next {
            self.next = uid + 1;
        }
    }

    /// Look up the UID for a named element.
    pub fn get_uid(&self, ident: &str) -> Option<i32> {
        self.reverse.get(ident).copied()
    }
}

impl Default for UidManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uid_manager_alloc_named() {
        let mut mgr = UidManager::new();
        let uid1 = mgr.alloc("population");
        let uid2 = mgr.alloc("population");
        assert_eq!(uid1, uid2);

        let uid3 = mgr.alloc("birth_rate");
        assert_ne!(uid1, uid3);

        // Third call for the same name still returns the original UID
        let uid4 = mgr.alloc("population");
        assert_eq!(uid1, uid4);
    }

    #[test]
    fn test_uid_manager_alloc_unnamed() {
        let mut mgr = UidManager::new();
        let uid1 = mgr.alloc("");
        let uid2 = mgr.alloc("");
        let uid3 = mgr.alloc("");
        assert_ne!(uid1, uid2);
        assert_ne!(uid2, uid3);
        assert_ne!(uid1, uid3);
    }

    #[test]
    fn test_uid_manager_add_existing() {
        let mut mgr = UidManager::new();
        mgr.add(10, "stock_a");
        mgr.add(20, "stock_b");

        assert_eq!(mgr.get_uid("stock_a"), Some(10));
        assert_eq!(mgr.get_uid("stock_b"), Some(20));
        assert_eq!(mgr.get_uid("nonexistent"), None);
    }

    #[test]
    fn test_uid_manager_starts_at_one() {
        let mut mgr = UidManager::new();
        let uid = mgr.alloc("first");
        assert_eq!(uid, 1);

        // UID 0 is never allocated, even across many allocations
        let mut all_uids = vec![uid];
        for i in 0..50 {
            all_uids.push(mgr.alloc(&format!("var_{i}")));
        }
        assert!(!all_uids.contains(&0));
    }

    #[test]
    fn test_uid_manager_alloc_after_add() {
        let mut mgr = UidManager::new();
        mgr.add(100, "existing_var");

        // Allocating a new name should not collide with the seeded UID
        let new_uid = mgr.alloc("new_var");
        assert!(new_uid > 100);
        assert_ne!(new_uid, 100);

        // The seeded variable should still be retrievable
        assert_eq!(mgr.get_uid("existing_var"), Some(100));
        assert_eq!(mgr.get_uid("new_var"), Some(new_uid));
    }

    #[test]
    fn test_uid_manager_add_zero_ignored() {
        let mut mgr = UidManager::new();
        mgr.add(0, "should_be_ignored");
        assert_eq!(mgr.get_uid("should_be_ignored"), None);

        // next should still be 1
        let uid = mgr.alloc("first");
        assert_eq!(uid, 1);
    }

    #[test]
    fn test_uid_manager_add_overwrites_reverse_mapping() {
        let mut mgr = UidManager::new();
        mgr.add(5, "var_a");
        mgr.add(10, "var_a");
        assert_eq!(mgr.get_uid("var_a"), Some(10));
    }
}
