//! DynamoStore integration test against DynamoDB Local. Ignored by default because it needs Docker:
//!
//! ```sh
//! docker run --rm -d -p 8000:8000 --name choosey-dynamo amazon/dynamodb-local
//! cargo test -p choosey-store --test dynamo_local -- --ignored
//! docker stop choosey-dynamo
//! ```
//!
//! Set `DYNAMODB_ENDPOINT` to use something other than `http://localhost:8000`.

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};
use choosey_store::{DynamoStore, Store};

fn local_client() -> Client {
    let endpoint =
        std::env::var("DYNAMODB_ENDPOINT").unwrap_or_else(|_| "http://localhost:8000".into());
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .endpoint_url(endpoint)
        .region(Region::new("us-east-1"))
        // DynamoDB Local accepts any credentials.
        .credentials_provider(Credentials::new(
            "local",
            "local",
            None,
            None,
            "dynamodb-local",
        ))
        .build();
    Client::from_conf(config)
}

#[tokio::test]
#[ignore = "needs DynamoDB Local (docker); see the module docs"]
async fn dynamo_store_conformance() {
    let client = local_client();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let table = format!("choosey-test-{nanos}");

    // Same shape as production: on-demand, string partition key `pk`, nothing else.
    client
        .create_table()
        .table_name(&table)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .expect("create table (is DynamoDB Local running?)");

    let store = DynamoStore::new(client.clone(), &table);
    choosey_store::conformance::check(&store as &dyn Store, "dyn").await;

    client
        .delete_table()
        .table_name(&table)
        .send()
        .await
        .unwrap();
}
