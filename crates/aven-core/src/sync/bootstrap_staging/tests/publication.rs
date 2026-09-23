use super::*;

impl Fixture {
    fn publication(&self) -> Publication {
        self.seed
            .prepare_bootstrap_publication(&self.package, &self.key)
            .unwrap()
    }

    fn publish_request<'a>(
        &self,
        publication: &'a Publication,
        epoch: u64,
    ) -> PublishBootstrap<'a> {
        PublishBootstrap {
            bootstrap_id: self.id,
            descriptor_commitment: self.commitment(),
            epoch,
            record: publication.record(),
        }
    }

    async fn publish(
        &self,
        publication: &Publication,
        epoch: u64,
    ) -> anyhow::Result<PublicationOutcome> {
        self.server
            .publish_bootstrap(
                &self.auth(),
                self.publish_request(publication, epoch),
                PublicationPolicy::default(),
            )
            .await
    }

    async fn unpublished(&self) {
        let mut conn = self.server.acquire_reader().await.unwrap();
        for table in [
            "server_bootstrap_publication",
            "server_e2ee_membership_head",
            "server_e2ee_allocator",
            "server_bootstrap_prefix",
            "server_e2ee_images",
            "server_e2ee_image_chunks",
            "server_e2ee_image_parents",
            "server_e2ee_image_references",
        ] {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        let genesis_only: bool = sqlx::query_scalar("SELECT genesis_only FROM server_seed_claim")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert!(genesis_only);
    }
}

mod authorization;
mod completeness;
mod crash;
mod lifecycle;
