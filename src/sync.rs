use std::sync::{LockResult, PoisonError};

/// The guard (or value) of a lock operation, also when the lock is poisoned. A poisoned lock
/// means another thread panicked, which ends the run anyway; the data under each lock stays
/// usable, so a poisoned lock is never an error of its own.
pub fn unpoisoned<G>(result: LockResult<G>) -> G {
    result.unwrap_or_else(PoisonError::into_inner)
}
