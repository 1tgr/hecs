// Copyright 2019 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use crate::archetype::Archetype;
use crate::{Component, MissingComponent};

#[cfg_attr(feature = "single_threaded", allow(dead_code))]
mod atomic {
    use crate::borrow::UNIQUE_BIT;
    use core::sync::atomic::{AtomicUsize, Ordering};

    pub struct Borrow(AtomicUsize);

    impl Borrow {
        pub const fn new() -> Self {
            Self(AtomicUsize::new(0))
        }

        pub fn borrow(&self) -> bool {
            let value = self.0.fetch_add(1, Ordering::Acquire).wrapping_add(1);
            if value == 0 {
                // Wrapped, this borrow is invalid!
                core::panic!()
            }
            if value & UNIQUE_BIT != 0 {
                self.0.fetch_sub(1, Ordering::Release);
                false
            } else {
                true
            }
        }

        pub fn borrow_mut(&self) -> bool {
            self.0
                .compare_exchange(0, UNIQUE_BIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        }

        pub fn release(&self) {
            let value = self.0.fetch_sub(1, Ordering::Release);
            debug_assert!(value != 0, "unbalanced release");
            debug_assert!(value & UNIQUE_BIT == 0, "shared release of unique borrow");
        }

        pub fn release_mut(&self) {
            let value = self.0.fetch_and(!UNIQUE_BIT, Ordering::Release);
            debug_assert_ne!(value & UNIQUE_BIT, 0, "unique release of shared borrow");
        }
    }
}

#[cfg_attr(not(feature = "single_threaded"), allow(dead_code))]
mod single_threaded {
    use crate::borrow::UNIQUE_BIT;
    use core::cell::Cell;

    pub struct Borrow(Cell<usize>);

    impl Borrow {
        pub const fn new() -> Self {
            Self(Cell::new(0))
        }

        pub fn borrow(&self) -> bool {
            let value = self.0.get().wrapping_add(1);
            if value == 0 {
                // Wrapped, this borrow is invalid!
                core::panic!()
            }
            if value & UNIQUE_BIT != 0 {
                false
            } else {
                self.0.set(value);
                true
            }
        }

        pub fn borrow_mut(&self) -> bool {
            if self.0.get() == 0 {
                self.0.set(UNIQUE_BIT);
                true
            } else {
                false
            }
        }

        pub fn release(&self) {
            let value = self.0.get();
            debug_assert!(value != 0, "unbalanced release");
            debug_assert!(value & UNIQUE_BIT == 0, "shared release of unique borrow");
            self.0.set(value - 1);
        }

        pub fn release_mut(&self) {
            let value = self.0.get();
            debug_assert_ne!(value & UNIQUE_BIT, 0, "unique release of shared borrow");
            self.0.set(value & !UNIQUE_BIT);
        }
    }
}

#[cfg(not(feature = "single_threaded"))]
pub use atomic::Borrow;

#[cfg(feature = "single_threaded")]
pub use single_threaded::Borrow;

const UNIQUE_BIT: usize = !(usize::max_value() >> 1);

/// Shared borrow of an entity's component
#[derive(Clone)]
pub struct Ref<'a, T: Component> {
    archetype: &'a Archetype,
    target: NonNull<T>,
}

impl<'a, T: Component> Ref<'a, T> {
    pub(crate) unsafe fn new(
        archetype: &'a Archetype,
        index: u32,
    ) -> Result<Self, MissingComponent> {
        let target = NonNull::new_unchecked(
            archetype
                .get::<T>()
                .ok_or_else(MissingComponent::new::<T>)?
                .as_ptr()
                .add(index as usize),
        );
        archetype.borrow::<T>();
        Ok(Self { archetype, target })
    }
}

unsafe impl<T: Component> Send for Ref<'_, T> {}
unsafe impl<T: Component> Sync for Ref<'_, T> {}

impl<'a, T: Component> Drop for Ref<'a, T> {
    fn drop(&mut self) {
        self.archetype.release::<T>();
    }
}

impl<'a, T: Component> Deref for Ref<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { self.target.as_ref() }
    }
}

/// Unique borrow of an entity's component
pub struct RefMut<'a, T: Component> {
    archetype: &'a Archetype,
    target: NonNull<T>,
}

impl<'a, T: Component> RefMut<'a, T> {
    pub(crate) unsafe fn new(
        archetype: &'a Archetype,
        index: u32,
    ) -> Result<Self, MissingComponent> {
        let target = NonNull::new_unchecked(
            archetype
                .get::<T>()
                .ok_or_else(MissingComponent::new::<T>)?
                .as_ptr()
                .add(index as usize),
        );
        archetype.borrow_mut::<T>();
        Ok(Self { archetype, target })
    }
}

unsafe impl<T: Component> Send for RefMut<'_, T> {}
unsafe impl<T: Component> Sync for RefMut<'_, T> {}

impl<'a, T: Component> Drop for RefMut<'a, T> {
    fn drop(&mut self) {
        self.archetype.release_mut::<T>();
    }
}

impl<'a, T: Component> Deref for RefMut<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { self.target.as_ref() }
    }
}

impl<'a, T: Component> DerefMut for RefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { self.target.as_mut() }
    }
}

/// Handle to an entity with any component types
#[derive(Copy, Clone)]
pub struct EntityRef<'a> {
    archetype: Option<&'a Archetype>,
    index: u32,
}

impl<'a> EntityRef<'a> {
    /// Construct a `Ref` for an entity with no components
    pub(crate) fn empty() -> Self {
        Self {
            archetype: None,
            index: 0,
        }
    }

    pub(crate) unsafe fn new(archetype: &'a Archetype, index: u32) -> Self {
        Self {
            archetype: Some(archetype),
            index,
        }
    }

    /// Borrow the component of type `T`, if it exists
    ///
    /// Panics if the component is already uniquely borrowed from another entity with the same
    /// components.
    pub fn get<T: Component>(&self) -> Option<Ref<'a, T>> {
        Some(unsafe { Ref::new(self.archetype?, self.index).ok()? })
    }

    /// Uniquely borrow the component of type `T`, if it exists
    ///
    /// Panics if the component is already borrowed from another entity with the same components.
    pub fn get_mut<T: Component>(&self) -> Option<RefMut<'a, T>> {
        Some(unsafe { RefMut::new(self.archetype?, self.index).ok()? })
    }
}

unsafe impl<'a> Send for EntityRef<'a> {}
unsafe impl<'a> Sync for EntityRef<'a> {}
