// Copyright (c) 2024 Contributors to the Eclipse Foundation
//
// See the NOTICE file(s) distributed with this work for additional
// information regarding copyright ownership.
//
// This program and the accompanying materials are made available under the
// terms of the Apache Software License 2.0 which is available at
// https://www.apache.org/licenses/LICENSE-2.0, or the MIT license
// which is available at https://opensource.org/licenses/MIT.
//
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::fmt::Debug;

use crate::pointer_trait::PointerTrait;

/// Trait that allows to use typed pointers as generic arguments for structs.
pub trait GenericPointer {
    /// The underlying pointer type.
    type Type<T: Debug>: PointerTrait<T> + Debug;
}