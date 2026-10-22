use anyhow::Result;
use redis::AsyncCommands;

use super::AUTHED_USERS_KEY;

pub async fn is_authed(user_id: u64) -> Result<bool> {
    let mut con = crate::get_connection().await;
    Ok(con.sismember(AUTHED_USERS_KEY, user_id).await?)
}

pub async fn add_authed(user_id: u64) -> Result<()> {
    let mut con = crate::get_connection().await;
    () = con.sadd(AUTHED_USERS_KEY, user_id).await?;

    Ok(())
}

pub async fn count_authed() -> Result<usize> {
    let mut con = crate::get_connection().await;
    Ok(con.scard(AUTHED_USERS_KEY).await?)
}
