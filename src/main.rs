mod commands;
use anyhow::anyhow;
use shuttle_secrets::SecretStore;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

// use ::serde::{Deserialize, Serialize};
// use ::serde_json::Result;

use serenity::{
    async_trait, builder::*, framework::standard::macros::group, framework::StandardFramework,
    http::Http, model::gateway::Ready, model::prelude::*, prelude::*,
};

use strsim::*;

use circular_buffer::CircularBuffer;

// Import commands

use crate::commands::clearly::*;

const MESSAGE_TIMER: u64 = 18;
const MAX_MESSAGE_COUNTER: u32 = 5;
const MESSAGE_SIMILARITY: f64 = 0.42;
const TIMEOUT_DURATION: i64 = 5; // 10 seconds
const MAX_STRIKE_COUNT: u32 = 5;

struct Handler;

struct UserSpam {
    last_message: Instant,
    message_count: u32,
    buffer: CircularBuffer<{ MAX_MESSAGE_COUNTER as usize }, String>,
    strike_count: u32,
    actioned_strike_count: u32,
}

impl UserSpam {
    pub fn new() -> UserSpam {
        UserSpam {
            last_message: Instant::now(),
            message_count: 0,
            buffer: CircularBuffer::<{ MAX_MESSAGE_COUNTER as usize }, String>::new(),
            strike_count: 0,
            actioned_strike_count: 0,
        }
    }
    pub fn print_debug(&mut self) {
        println!(
            "--- \n last message was at : {} \n message count : {} \n buffer : {}",
            self.last_message.elapsed().as_secs(),
            self.message_count,
            self.buffer.to_vec().concat()
        )
    }
}

struct BotState;

impl TypeMapKey for BotState {
    type Value = HashMap<u64, UserSpam>;
}

// // Function to timeout a user
// async fn timeout_user(ctx: &Context, guild_id: GuildId, user_id: UserId, timeout_duration: u32) -> serenity::Result<Member> {
//     let http = &ctx.http;

//     // Calculate the timeout expiration timestamp
//     let timeout_expiration = chrono::Utc::now() + chrono::Duration::days(timeout_duration as i64);

//     // Create the payload with the timeout duration
//     let payload = json!({
//         "communication_disabled_until": timeout_expiration.to_rfc3339()
//     });

//     // Send the PATCH request to the Discord API
//     let member = http.edit_member(guild_id.0, user_id.0, )
//         .json(&payload)
//         .await?;

//     Ok(member)
// }

fn has_repeated_words(text: &str) -> bool {
    let mut word_counts: HashMap<&str, usize> = HashMap::new();
    let mut emote_counts: HashMap<&str, usize> = HashMap::new();

    for element in text.split(|c: char| c.is_whitespace() || c == '<' || c == '>') {
        if !element.is_empty() {
            // Check if the element is an emote
            if element.starts_with(':') && element.ends_with(':') {
                // Increment the count for the current emote
                *emote_counts.entry(element).or_insert(0) += 1;

                // Check if the count exceeds the threshold (6)
                if *emote_counts.get(element).unwrap() > 6 {
                    return true;
                }
            } else {
                // Increment the count for the current word
                *word_counts.entry(element).or_insert(0) += 1;

                // Check if the count exceeds the threshold (6)
                if *word_counts.get(element).unwrap() > 6 {
                    return true;
                }
            }
        }
    }

    false
}

pub async fn check_token_spam(ctx: Context, msg: Message) -> bool {
    if has_repeated_words(&msg.content) {
        if let Err(why) = msg.delete(&ctx.http).await {
            eprintln!("Error deleting message: {:?}", why);
        }
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                format!(
                    "{}, Stop spamming emotes you fucking moron <:omE:1104812023835005008>",
                    msg.author
                ),
            )
            .await;
        {
            let mut data = ctx.data.write().await;

            // Update the spam tracker with the new message
            let spam_info = data
                .get_mut::<BotState>()
                .unwrap()
                .entry(msg.author.id.0)
                .or_insert(UserSpam::new());
            if spam_info.strike_count >= MAX_STRIKE_COUNT {
                spam_info.actioned_strike_count += 1;
                let timeout_duration = (spam_info.actioned_strike_count + 1) * 60;
                if let Some(dm_channel) = msg.author.id.create_dm_channel(&ctx.http).await.ok() {
                    if let Some(member) = msg.member(&ctx.http).await.ok() {
                        let timeout_expiration =
                            chrono::Utc::now() + chrono::Duration::seconds(timeout_duration as i64);
                        let timeout_res = member
                            .edit(&ctx.http, |builder: &mut EditMember| {
                                builder.disable_communication_until(timeout_expiration.to_rfc3339())
                            })
                            .await;
                        // handle timeout error
                        if timeout_res.is_err() {
                            println!("Timeout Error : {}", timeout_res.unwrap_err());
                        }
                        let _ = dm_channel.say(&ctx.http, format!("Enough with the spam ! This is your {}th strike, and your {}th actionable strike, timed out for {} seconds",spam_info.strike_count,spam_info.actioned_strike_count, timeout_duration) ).await;
                    }
                }
                spam_info.strike_count = 0;
            } else {
                spam_info.strike_count += 1;
            }
        }
        return true;
    }
    return false;
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        let user_id = msg.author.id.0;

        let bot_id = ctx.cache.current_user_id().0;

        // Ignore messages sent by the bot itself
        if user_id == bot_id {
            return;
        }
        println!("received message");

        if check_token_spam(ctx, msg).await {
            return;
        }

        {
            let mut data = ctx.data.write().await;

            // Update the spam tracker with the new message
            let spam_info = data
                .get_mut::<BotState>()
                .unwrap()
                .entry(user_id)
                .or_insert(UserSpam::new());

            // if last message has been done after a certain duration, reset the counter :
            if spam_info.last_message.elapsed() > Duration::from_secs(MESSAGE_TIMER) {
                spam_info.message_count = 0;
            }

            spam_info.last_message = Instant::now();
            spam_info.message_count += 1;
            spam_info.buffer.push_back(msg.content.to_string());

            spam_info.print_debug();

            let elapsed = spam_info.last_message.elapsed();
            if elapsed < Duration::from_secs(MESSAGE_TIMER)
                && spam_info.message_count >= MAX_MESSAGE_COUNTER
            {
                // User is spamming, first compare similarity of messages
                let message_0 = spam_info.buffer.get(0).unwrap();
                let mut dist: f64 = 0.0;
                for i in 1..MAX_MESSAGE_COUNTER {
                    let strdst = jaro(message_0, spam_info.buffer.get(i as usize).unwrap());
                    println!("strdst of 0,{} = {}", i, strdst);
                    dist += strdst;
                }
                dist /= MAX_MESSAGE_COUNTER as f64; // normalize the dist
                println!("{}", dist);
                if dist > MESSAGE_SIMILARITY {
                    //let _ = msg.channel_id.say(&ctx.http, format!("{} has been detected as a spammer!", msg.author)).await;
                    // Send a direct message to the user
                    if let Some(dm_channel) = msg.author.id.create_dm_channel(&ctx.http).await.ok()
                    {
                        //let member_timeout = EditMember()
                        //ctx.http.edit_member(guild_id, user_id, , Ok("Spam".to_string()));
                        if let Some(member) = msg.member(&ctx.http).await.ok() {
                            let timeout_expiration = chrono::Utc::now()
                                + chrono::Duration::seconds(TIMEOUT_DURATION as i64);
                            let timeout_res = member
                                .edit(&ctx.http, |builder: &mut EditMember| {
                                    builder.disable_communication_until(
                                        timeout_expiration.to_rfc3339(),
                                    )
                                })
                                .await;
                            // handle timeout error
                            if timeout_res.is_err() {
                                println!("Timeout Error : {}", timeout_res.unwrap_err());
                            }
                            let _ = dm_channel.say(&ctx.http, "You have been detected as a spammer! This is your nth strike this session ,timed out for ..").await;
                            {
                                data.get_mut::<BotState>()
                                    .unwrap()
                                    .entry(user_id)
                                    .and_modify(|spam_info_v| {
                                        spam_info_v.buffer.clear();
                                        spam_info_v.message_count = 0;
                                    });
                            }
                        }
                    }
                }
            }
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);

        // tokio::spawn(async move {
        //     loop {
        //         // Replace CHANNEL_ID with the ID of the desired channel to send the command
        //         let channel_id = serenity::model::id::ChannelId(1117948861730799740);

        //         // Replace "YOUR_COMMAND" with the actual command you want to send
        //         if let Err(why) = channel_id.say(&ctx.http, "/bump").await {
        //             eprintln!("Error sending message: {:?}", why);
        //         }

        //         // Wait for 40 minutes before sending the command again
        //         tokio::time::sleep(Duration::from_secs(1 * 60)).await;
        //     }
        // });
    }
}

#[group]
#[commands(clearly)]
struct General;

#[shuttle_runtime::main]
async fn serenity(
    #[shuttle_secrets::Secrets] secret_store: SecretStore,
) -> shuttle_serenity::ShuttleSerenity {
    // Login with your bot token
    let token = if let Some(token) = secret_store.get("DISCORD_TOKEN") {
        token
    } else {
        return Err(anyhow!("'DISCORD_TOKEN' was not found").into());
    };

    let http = Http::new(&token);

    // We will fetch your bot's owners and id
    let (owners, _bot_id) = match http.get_current_application_info().await {
        Ok(info) => {
            let mut owners = HashSet::new();
            owners.insert(info.owner.id);

            (owners, info.id)
        }
        Err(why) => panic!("Could not access application info: {:?}", why),
    };

    // Create framework and intents :
    let framework = StandardFramework::new()
        .configure(|c| c.owners(owners).prefix(">"))
        .group(&GENERAL_GROUP);

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    // Create a new instance of the bot
    let mut client = Client::builder(&token, intents)
        .framework(framework)
        .event_handler(Handler)
        .type_map_insert::<BotState>(HashMap::new())
        .await
        .expect("Error creating client");

    // Start the bot
    if let Err(why) = client.start().await {
        eprintln!("An error occurred while running the client: {:?}", why);
    }
    Ok(client.into())
}
