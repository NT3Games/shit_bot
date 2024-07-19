use anyhow::Result;
use redis::AsyncCommands;

pub const AUTHED_USERS_KEY: &str = "shit_bot_authed_users";

pub async fn is_authed(user_id: u64) -> Result<bool> {
    let mut con = crate::get_client().await.get_async_connection().await?;
    Ok(con.sismember(AUTHED_USERS_KEY, user_id).await?)
}

pub async fn add_authed(user_id: u64) -> Result<()> {
    let mut con = crate::get_client().await.get_async_connection().await?;
    con.sadd(AUTHED_USERS_KEY, user_id).await?;

    Ok(())
}
