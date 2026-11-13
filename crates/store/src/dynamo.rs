//! DynamoDB store: one item per decision.
//!
//! The item is the serde serialisation of [`Decision`] (via `serde_dynamo`) plus the partition key
//! `pk = "DEC#<view_id>"`. Writes are conditional on the `version` attribute, which gives
//! optimistic concurrency without locks.

use std::collections::HashMap;

use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::types::AttributeValue;
use choosey_core::model::Decision;

use crate::{Store, StoreError};

/// Name of the table's partition key attribute.
pub const PK: &str = "pk";

pub struct DynamoStore {
    client: Client,
    table: String,
}

impl DynamoStore {
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// Build a client from the standard AWS environment (region, credentials).
    pub async fn from_env(table: impl Into<String>) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self::new(Client::new(&config), table)
    }

    fn key(view_id: &str) -> AttributeValue {
        AttributeValue::S(format!("DEC#{view_id}"))
    }

    fn to_item(decision: &Decision) -> Result<HashMap<String, AttributeValue>, StoreError> {
        let mut item: HashMap<String, AttributeValue> = serde_dynamo::to_item(decision)
            .map_err(|e| StoreError::Backend(format!("serialise: {e}")))?;
        item.insert(PK.to_string(), Self::key(&decision.view_id));
        Ok(item)
    }
}

/// True if the SDK error is DynamoDB's "the condition expression was false".
fn is_condition_failed<E: ProvideErrorMetadata, R>(err: &SdkError<E, R>) -> bool {
    err.as_service_error()
        .and_then(|e| e.code())
        .is_some_and(|code| code == "ConditionalCheckFailedException")
}

/// Turn any other SDK error into a loggable backend error. The SDK's messages describe the
/// failure (throttling, permissions...) and don't echo our keys.
fn backend<E: std::error::Error + 'static, R: std::fmt::Debug>(err: SdkError<E, R>) -> StoreError {
    StoreError::Backend(DisplayErrorContext(&err).to_string())
}

#[async_trait]
impl Store for DynamoStore {
    async fn create(&self, decision: &Decision) -> Result<(), StoreError> {
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(Self::to_item(decision)?))
            .condition_expression("attribute_not_exists(#pk)")
            .expression_attribute_names("#pk", PK)
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if is_condition_failed(&e) => Err(StoreError::AlreadyExists),
            Err(e) => Err(backend(e)),
        }
    }

    async fn get(&self, view_id: &str) -> Result<Option<Decision>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key(PK, Self::key(view_id))
            // Read-your-writes: the page shown after a redirect must include the change just made.
            .consistent_read(true)
            .send()
            .await
            .map_err(backend)?;
        match output.item {
            // `pk` isn't a field of Decision; serde ignores unknown attributes.
            Some(item) => serde_dynamo::from_item(item)
                .map(Some)
                .map_err(|e| StoreError::Backend(format!("deserialise: {e}"))),
            None => Ok(None),
        }
    }

    async fn put(&self, decision: &Decision, expected_version: u64) -> Result<(), StoreError> {
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(Self::to_item(decision)?))
            // Fails if the item is missing (deleted) or someone else bumped the version.
            .condition_expression("#v = :expected")
            .expression_attribute_names("#v", "version")
            .expression_attribute_values(
                ":expected",
                AttributeValue::N(expected_version.to_string()),
            )
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if is_condition_failed(&e) => Err(StoreError::Conflict),
            Err(e) => Err(backend(e)),
        }
    }

    async fn delete(&self, view_id: &str) -> Result<(), StoreError> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key(PK, Self::key(view_id))
            .send()
            .await
            .map_err(backend)?;
        Ok(())
    }
}
