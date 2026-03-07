// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::block::{BlockAPI, VerifiedBlock};

#[derive(Clone)]
pub struct BlockInfo {
    pub block: VerifiedBlock,
    pub committed: bool,
    pub included: bool,
}

impl BlockInfo {
    pub fn new(block: VerifiedBlock) -> Self {
        assert!(block.round() > 0, "Genesis blocks should not be cached.");
        Self {
            block,
            committed: false,
            included: false,
        }
    }
}
