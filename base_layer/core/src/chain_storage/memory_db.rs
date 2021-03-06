// Copyright 2019. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

//! This is a memory-based blockchain database, generally only useful for testing purposes

use crate::{
    blocks::{Block, BlockHeader},
    chain_storage::{
        blockchain_database::{BlockchainBackend, MutableMmrState},
        db_transaction::{DbKey, DbKeyValuePair, DbTransaction, DbValue, MetadataValue, MmrTree, WriteOperation},
        error::ChainStorageError,
    },
};
use digest::Digest;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use tari_mmr::{
    functions::prune_mutable_mmr,
    Hash as MmrHash,
    MerkleChangeTracker,
    MerkleChangeTrackerConfig,
    MerkleCheckPoint,
    MerkleProof,
    MutableMmr,
    MutableMmrLeafNodes,
};
use tari_transactions::{
    transaction::{TransactionKernel, TransactionOutput},
    types::HashOutput,
};
use tari_utilities::hash::Hashable;

/// A generic struct for storing node objects in the BlockchainDB that also form part of an MMR. The index field makes
/// reverse lookups (find by hash) possible.
#[derive(Debug)]
struct MerkleNode<T> {
    index: usize,
    value: T,
}

#[derive(Debug)]
struct InnerDatabase<D>
where D: Digest
{
    metadata: HashMap<u32, MetadataValue>,
    headers: HashMap<u64, BlockHeader>,
    block_hashes: HashMap<HashOutput, u64>,
    utxos: HashMap<HashOutput, MerkleNode<TransactionOutput>>,
    stxos: HashMap<HashOutput, MerkleNode<TransactionOutput>>,
    kernels: HashMap<HashOutput, TransactionKernel>,
    orphans: HashMap<HashOutput, Block>,
    // Define MMRs to use both a memory-backed base and a memory-backed pruned MMR
    utxo_mmr: MerkleChangeTracker<D, Vec<MmrHash>, Vec<MerkleCheckPoint>>,
    kernel_mmr: MerkleChangeTracker<D, Vec<MmrHash>, Vec<MerkleCheckPoint>>,
    range_proof_mmr: MerkleChangeTracker<D, Vec<MmrHash>, Vec<MerkleCheckPoint>>,
}

/// A memory-backed blockchain database. The data is stored in RAM; and so all data will be lost when the program
/// terminates. Thus this DB is intended for testing purposes. It's also not very efficient since a single Mutex
/// protects the entire database. Again: testing.
#[derive(Default, Debug)]
pub struct MemoryDatabase<D>
where D: Digest
{
    db: Arc<RwLock<InnerDatabase<D>>>,
}

impl<D> MemoryDatabase<D>
where D: Digest
{
    pub fn new(mct_config: MerkleChangeTrackerConfig) -> Self {
        let utxo_mmr =
            MerkleChangeTracker::<D, _, _>::new(MutableMmr::new(Vec::new()), Vec::new(), mct_config).unwrap();
        let kernel_mmr =
            MerkleChangeTracker::<D, _, _>::new(MutableMmr::new(Vec::new()), Vec::new(), mct_config).unwrap();
        let range_proof_mmr =
            MerkleChangeTracker::<D, _, _>::new(MutableMmr::new(Vec::new()), Vec::new(), mct_config).unwrap();
        Self {
            db: Arc::new(RwLock::new(InnerDatabase {
                metadata: HashMap::default(),
                headers: HashMap::default(),
                block_hashes: HashMap::default(),
                utxos: HashMap::default(),
                stxos: HashMap::default(),
                kernels: HashMap::default(),
                orphans: HashMap::default(),
                utxo_mmr,
                kernel_mmr,
                range_proof_mmr,
            })),
        }
    }

    pub(self) fn db_access(&self) -> Result<RwLockReadGuard<InnerDatabase<D>>, ChainStorageError> {
        self.db
            .read()
            .map_err(|e| ChainStorageError::AccessError(e.to_string()))
    }
}

impl<D> BlockchainBackend for MemoryDatabase<D>
where D: Digest + Send + Sync
{
    fn write(&self, tx: DbTransaction) -> Result<(), ChainStorageError> {
        let mut db = self
            .db
            .write()
            .map_err(|e| ChainStorageError::AccessError(e.to_string()))?;
        // Not **really** atomic, but..
        // Hashmap insertions don't typically fail and b) MemoryDB should not be used for production anyway.
        for op in tx.operations.into_iter() {
            match op {
                WriteOperation::Insert(insert) => match insert {
                    DbKeyValuePair::Metadata(k, v) => {
                        let key = k as u32;
                        if db.metadata.contains_key(&key) {
                            return Err(ChainStorageError::InvalidOperation("Duplicate key".to_string()));
                        }
                        db.metadata.insert(key, v);
                    },
                    DbKeyValuePair::BlockHeader(k, v) => {
                        if db.headers.contains_key(&k) {
                            return Err(ChainStorageError::InvalidOperation("Duplicate key".to_string()));
                        }
                        db.block_hashes.insert(v.hash(), k);
                        db.headers.insert(k, *v);
                    },
                    DbKeyValuePair::UnspentOutput(k, v, update_mmr) => {
                        if db.utxos.contains_key(&k) {
                            return Err(ChainStorageError::InvalidOperation("Duplicate key".to_string()));
                        }
                        let proof_hash = v.proof().hash();
                        if update_mmr {
                            db.utxo_mmr.push(&k)?;
                            db.range_proof_mmr.push(&proof_hash)?;
                        }
                        if let Some(index) = db.range_proof_mmr.find_leaf_index(&proof_hash)? {
                            let v = MerkleNode { index, value: *v };
                            db.utxos.insert(k, v);
                        }
                    },
                    DbKeyValuePair::TransactionKernel(k, v, update_mmr) => {
                        if db.kernels.contains_key(&k) {
                            return Err(ChainStorageError::InvalidOperation("Duplicate key".to_string()));
                        }
                        if update_mmr {
                            db.kernel_mmr.push(&k)?;
                        }
                        db.kernels.insert(k, *v);
                    },
                    DbKeyValuePair::OrphanBlock(k, v) => {
                        if db.orphans.contains_key(&k) {
                            return Err(ChainStorageError::InvalidOperation("Duplicate key".to_string()));
                        }
                        db.orphans.insert(k, *v);
                    },
                },
                WriteOperation::Delete(delete) => match delete {
                    DbKey::Metadata(_) => {}, // no-op
                    DbKey::BlockHeader(k) => {
                        db.headers.remove(&k).and_then(|v| db.block_hashes.remove(&v.hash()));
                    },
                    DbKey::BlockHash(hash) => {
                        db.block_hashes.remove(&hash).and_then(|i| db.headers.remove(&i));
                    },
                    DbKey::UnspentOutput(k) => {
                        db.utxos.remove(&k);
                    },
                    DbKey::SpentOutput(k) => {
                        db.stxos.remove(&k);
                    },
                    DbKey::TransactionKernel(k) => {
                        db.kernels.remove(&k);
                    },
                    DbKey::OrphanBlock(k) => {
                        db.orphans.remove(&k);
                    },
                },
                WriteOperation::Spend(key) => match key {
                    DbKey::UnspentOutput(hash) => {
                        let moved = spend_utxo(&mut db, hash);
                        if !moved {
                            return Err(ChainStorageError::UnspendableInput);
                        }
                    },
                    _ => return Err(ChainStorageError::InvalidOperation("Only UTXOs can be spent".into())),
                },
                WriteOperation::UnSpend(key) => match key {
                    DbKey::SpentOutput(hash) => {
                        let moved = unspend_stxo(&mut db, hash);
                        if !moved {
                            return Err(ChainStorageError::UnspendError);
                        }
                    },
                    _ => return Err(ChainStorageError::InvalidOperation("Only STXOs can be unspent".into())),
                },
                WriteOperation::CreateMmrCheckpoint(tree) => match tree {
                    MmrTree::Kernel => db
                        .kernel_mmr
                        .commit()
                        .map_err(|e| ChainStorageError::AccessError(e.to_string()))?,
                    MmrTree::Utxo => db
                        .utxo_mmr
                        .commit()
                        .map_err(|e| ChainStorageError::AccessError(e.to_string()))?,
                    MmrTree::RangeProof => db
                        .range_proof_mmr
                        .commit()
                        .map_err(|e| ChainStorageError::AccessError(e.to_string()))?,
                },
                WriteOperation::RewindMmr(tree, steps_back) => match tree {
                    MmrTree::Kernel => {
                        if steps_back == 0 {
                            db.kernel_mmr
                                .reset()
                                .map_err(|e| ChainStorageError::AccessError(e.to_string()))?;
                        } else {
                            db.kernel_mmr
                                .rewind(steps_back)
                                .map_err(|e| ChainStorageError::AccessError(e.to_string()))?;
                        }
                    },
                    MmrTree::Utxo => {
                        if steps_back == 0 {
                            db.utxo_mmr
                                .reset()
                                .map_err(|e| ChainStorageError::AccessError(e.to_string()))?;
                        } else {
                            db.utxo_mmr
                                .rewind(steps_back)
                                .map_err(|e| ChainStorageError::AccessError(e.to_string()))?;
                        }
                    },
                    MmrTree::RangeProof => {
                        if steps_back == 0 {
                            db.range_proof_mmr
                                .reset()
                                .map_err(|e| ChainStorageError::AccessError(e.to_string()))?;
                        } else {
                            db.range_proof_mmr
                                .rewind(steps_back)
                                .map_err(|e| ChainStorageError::AccessError(e.to_string()))?;
                        }
                    },
                },
            }
        }
        Ok(())
    }

    fn fetch(&self, key: &DbKey) -> Result<Option<DbValue>, ChainStorageError> {
        let db = self.db_access()?;
        let result = match key {
            DbKey::Metadata(k) => db
                .metadata
                .get(&(k.clone() as u32))
                .map(|v| DbValue::Metadata(v.clone())),
            DbKey::BlockHeader(k) => db.headers.get(k).map(|v| DbValue::BlockHeader(Box::new(v.clone()))),
            DbKey::BlockHash(hash) => db
                .block_hashes
                .get(hash)
                .and_then(|i| db.headers.get(i))
                .map(|v| DbValue::BlockHash(Box::new(v.clone()))),
            DbKey::UnspentOutput(k) => db
                .utxos
                .get(k)
                .map(|v| DbValue::UnspentOutput(Box::new(v.value.clone()))),
            DbKey::SpentOutput(k) => db.stxos.get(k).map(|v| DbValue::SpentOutput(Box::new(v.value.clone()))),
            DbKey::TransactionKernel(k) => db
                .kernels
                .get(k)
                .map(|v| DbValue::TransactionKernel(Box::new(v.clone()))),
            DbKey::OrphanBlock(k) => db.orphans.get(k).map(|v| DbValue::OrphanBlock(Box::new(v.clone()))),
        };
        Ok(result)
    }

    fn contains(&self, key: &DbKey) -> Result<bool, ChainStorageError> {
        let db = self.db_access()?;
        let result = match key {
            DbKey::Metadata(_) => true,
            DbKey::BlockHeader(k) => db.headers.contains_key(k),
            DbKey::BlockHash(h) => db.block_hashes.contains_key(h),
            DbKey::UnspentOutput(k) => db.utxos.contains_key(k),
            DbKey::SpentOutput(k) => db.stxos.contains_key(k),
            DbKey::TransactionKernel(k) => db.kernels.contains_key(k),
            DbKey::OrphanBlock(k) => db.orphans.contains_key(k),
        };
        Ok(result)
    }

    fn fetch_mmr_root(&self, tree: MmrTree) -> Result<Vec<u8>, ChainStorageError> {
        let db = self.db_access()?;
        let root = match tree {
            MmrTree::Utxo => db.utxo_mmr.get_merkle_root()?,
            MmrTree::Kernel => db.kernel_mmr.get_merkle_root()?,
            MmrTree::RangeProof => db.range_proof_mmr.get_merkle_root()?,
        };
        Ok(root)
    }

    fn fetch_mmr_only_root(&self, tree: MmrTree) -> Result<Vec<u8>, ChainStorageError> {
        let db = self.db_access()?;
        let root = match tree {
            MmrTree::Utxo => db.utxo_mmr.get_mmr_only_root()?,
            MmrTree::Kernel => db.kernel_mmr.get_mmr_only_root()?,
            MmrTree::RangeProof => db.range_proof_mmr.get_mmr_only_root()?,
        };
        Ok(root)
    }

    fn calculate_mmr_root(
        &self,
        tree: MmrTree,
        additions: Vec<HashOutput>,
        deletions: Vec<HashOutput>,
    ) -> Result<Vec<u8>, ChainStorageError>
    {
        let db = self.db_access()?;
        let mut pruned_mmr = match tree {
            MmrTree::Utxo => prune_mutable_mmr(&db.utxo_mmr)?,
            MmrTree::Kernel => prune_mutable_mmr(&db.kernel_mmr)?,
            MmrTree::RangeProof => prune_mutable_mmr(&db.range_proof_mmr)?,
        };
        for hash in additions {
            pruned_mmr.push(&hash)?;
        }
        if let MmrTree::Utxo = tree {
            deletions.iter().for_each(|hash| {
                if let Some(node) = db.utxos.get(hash) {
                    pruned_mmr.delete(node.index as u32);
                }
            })
        }
        Ok(pruned_mmr.get_merkle_root()?)
    }

    /// Returns an MMR proof extracted from the full Merkle mountain range without trimming the MMR using the roaring
    /// bitmap
    fn fetch_mmr_proof(&self, tree: MmrTree, leaf_pos: usize) -> Result<MerkleProof, ChainStorageError> {
        let db = self.db_access()?;
        let proof = match tree {
            MmrTree::Utxo => MerkleProof::for_leaf_node(&db.utxo_mmr.mmr(), leaf_pos)?,
            MmrTree::Kernel => MerkleProof::for_leaf_node(&db.kernel_mmr.mmr(), leaf_pos)?,
            MmrTree::RangeProof => MerkleProof::for_leaf_node(&db.range_proof_mmr.mmr(), leaf_pos)?,
        };
        Ok(proof)
    }

    fn fetch_mmr_checkpoint(&self, tree: MmrTree, height: u64) -> Result<MerkleCheckPoint, ChainStorageError> {
        let db = self.db_access()?;
        let horizon_block = self.fetch_horizon_block_height()?;
        if height < horizon_block {
            return Err(ChainStorageError::BeyondPruningHorizon);
        }
        let index = (height - horizon_block) as usize;
        let cp = match tree {
            MmrTree::Kernel => db.kernel_mmr.get_checkpoint(index),
            MmrTree::Utxo => db.utxo_mmr.get_checkpoint(index),
            MmrTree::RangeProof => db.range_proof_mmr.get_checkpoint(index),
        };
        cp.map_err(|e| ChainStorageError::AccessError(format!("MMR Checkpoint error: {}", e.to_string())))
    }

    fn fetch_mmr_node(&self, tree: MmrTree, pos: u32) -> Result<(Vec<u8>, bool), ChainStorageError> {
        let db = self.db_access()?;
        let (hash, deleted) = match tree {
            MmrTree::Kernel => db.kernel_mmr.get_leaf_status(pos)?,
            MmrTree::Utxo => db.utxo_mmr.get_leaf_status(pos)?,
            MmrTree::RangeProof => db.range_proof_mmr.get_leaf_status(pos)?,
        };
        let hash = hash
            .ok_or(ChainStorageError::UnexpectedResult(format!(
                "A leaf node hash in the {} MMR tree was not found",
                tree
            )))?
            .clone();
        Ok((hash, deleted))
    }

    fn fetch_mmr_base_leaf_nodes(
        &self,
        tree: MmrTree,
        index: usize,
        count: usize,
    ) -> Result<MutableMmrState, ChainStorageError>
    {
        let db = self.db_access()?;
        let mmr_state = match tree {
            MmrTree::Kernel => MutableMmrState {
                total_leaf_count: db.kernel_mmr.get_base_leaf_count(),
                leaf_nodes: db.kernel_mmr.to_base_leaf_nodes(index, count)?,
            },
            MmrTree::Utxo => MutableMmrState {
                total_leaf_count: db.utxo_mmr.get_base_leaf_count(),
                leaf_nodes: db.utxo_mmr.to_base_leaf_nodes(index, count)?,
            },
            MmrTree::RangeProof => MutableMmrState {
                total_leaf_count: db.range_proof_mmr.get_base_leaf_count(),
                leaf_nodes: db.range_proof_mmr.to_base_leaf_nodes(index, count)?,
            },
        };
        Ok(mmr_state)
    }

    fn fetch_mmr_base_leaf_node_count(&self, tree: MmrTree) -> Result<usize, ChainStorageError> {
        let db = self.db_access()?;
        let mmr_state = match tree {
            MmrTree::Kernel => db.kernel_mmr.get_base_leaf_count(),
            MmrTree::Utxo => db.utxo_mmr.get_base_leaf_count(),
            MmrTree::RangeProof => db.range_proof_mmr.get_base_leaf_count(),
        };
        Ok(mmr_state)
    }

    fn assign_mmr(&self, tree: MmrTree, base_state: MutableMmrLeafNodes) -> Result<(), ChainStorageError> {
        let mut db = self
            .db
            .write()
            .map_err(|e| ChainStorageError::AccessError(e.to_string()))?;
        match tree {
            MmrTree::Kernel => db.kernel_mmr.assign(base_state)?,
            MmrTree::Utxo => db.utxo_mmr.assign(base_state)?,
            MmrTree::RangeProof => db.range_proof_mmr.assign(base_state)?,
        };
        Ok(())
    }

    /// Iterate over all the stored orphan blocks and execute the function `f` for each block.
    fn for_each_orphan<F>(&self, mut f: F) -> Result<(), ChainStorageError>
    where F: FnMut(Result<(HashOutput, Block), ChainStorageError>) {
        let db = self.db_access()?;
        for (key, val) in db.orphans.iter() {
            f(Ok((key.clone(), val.clone())));
        }
        Ok(())
    }

    /// The horizon block is the earliest block that we can return all data to reconstruct a full block
    fn fetch_horizon_block_height(&self) -> Result<u64, ChainStorageError> {
        let db = self.db_access()?;
        let tip_height = db.headers.len();
        let checkpoint_count = db.kernel_mmr.checkpoint_count()?;
        Ok((tip_height - checkpoint_count) as u64)
    }

    fn fetch_last_header(&self) -> Result<Option<BlockHeader>, ChainStorageError> {
        let db = self.db_access()?;
        let header_count = db.headers.len() as u64;
        if header_count >= 1 {
            let k = header_count - 1;
            Ok(db.headers.get(&k).map(|h| h.clone()))
        } else {
            Ok(None)
        }
    }
}

impl<D> Clone for MemoryDatabase<D>
where D: Digest
{
    fn clone(&self) -> Self {
        MemoryDatabase { db: self.db.clone() }
    }
}

impl<D> Default for InnerDatabase<D>
where D: Digest
{
    fn default() -> Self {
        let mct_config = MerkleChangeTrackerConfig {
            min_history_len: 900,
            max_history_len: 1000,
        };
        let utxo_mmr =
            MerkleChangeTracker::<D, _, _>::new(MutableMmr::new(Vec::new()), Vec::new(), mct_config).unwrap();
        let kernel_mmr =
            MerkleChangeTracker::<D, _, _>::new(MutableMmr::new(Vec::new()), Vec::new(), mct_config).unwrap();
        let range_proof_mmr =
            MerkleChangeTracker::<D, _, _>::new(MutableMmr::new(Vec::new()), Vec::new(), mct_config).unwrap();
        Self {
            metadata: HashMap::default(),
            headers: HashMap::default(),
            block_hashes: HashMap::default(),
            utxos: HashMap::default(),
            stxos: HashMap::default(),
            kernels: HashMap::default(),
            orphans: HashMap::default(),
            utxo_mmr,
            kernel_mmr,
            range_proof_mmr,
        }
    }
}

// This is a private helper function. When it is called, we are guaranteed to have a write lock on self.db
fn spend_utxo<D: Digest>(db: &mut RwLockWriteGuard<InnerDatabase<D>>, hash: HashOutput) -> bool {
    match db.utxos.remove(&hash) {
        None => false,
        Some(utxo) => {
            db.utxo_mmr.delete(utxo.index as u32);
            db.stxos.insert(hash, utxo);
            true
        },
    }
}

// This is a private helper function. When it is called, we are guaranteed to have a write lock on self.db. Unspend_stxo
// is only called for rewind operations and doesn't have to re-insert the utxo entry into the utxo_mmr as the MMR will
// be rolled back.
fn unspend_stxo<D: Digest>(db: &mut RwLockWriteGuard<InnerDatabase<D>>, hash: HashOutput) -> bool {
    match db.stxos.remove(&hash) {
        None => false,
        Some(stxo) => {
            db.utxos.insert(hash, stxo);
            true
        },
    }
}

#[cfg(test)]
mod test {
    use crate::chain_storage::{BlockchainBackend, MemoryDatabase, MmrTree};
    use croaring::Bitmap;
    use tari_mmr::{MerkleChangeTrackerConfig, MutableMmr, MutableMmrLeafNodes};
    use tari_transactions::{tari_amount::uT, tx, types::HashDigest};
    use tari_utilities::Hashable;

    /// Test the ability to assign a given state to the database MMR
    #[test]
    fn assign_mmr() {
        let mct = MerkleChangeTrackerConfig {
            min_history_len: 2,
            max_history_len: 3,
        };
        let db = MemoryDatabase::<HashDigest>::new(mct);
        // Build an MMR of transaction kernels
        let txs = vec![
            tx!(100_000 * uT, fee: 100 * uT),
            tx!(200_000 * uT, fee: 100 * uT),
            tx!(300_000 * uT, fee: 100 * uT),
            tx!(400_000 * uT, fee: 100 * uT),
            tx!(500_000 * uT, fee: 100 * uT),
        ];
        let hashes = txs.iter().map(|(tx, _, _)| tx.body.kernels()[0].hash()).collect();
        let mut deleted = Bitmap::create();
        // We aren't allowed to delete kernels, but this demonstrates that the deletions are assigned too
        deleted.add(3);
        let state = MutableMmrLeafNodes::new(hashes, deleted);
        // Create a local version of the MMR
        let mut mmr = MutableMmr::<HashDigest, _>::new(Vec::new());
        // Assign the state to the DB backend and compare roots
        mmr.assign(state.clone()).unwrap();
        let root = mmr.get_merkle_root().unwrap();
        db.assign_mmr(MmrTree::Kernel, state).unwrap();
        assert_eq!(db.fetch_mmr_root(MmrTree::Kernel).unwrap(), root);
    }
}
