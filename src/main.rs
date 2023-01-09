use std::env;

use serenity::async_trait;
use serenity::builder::CreateApplicationCommand;
use serenity::model::application::interaction::{Interaction, InteractionResponseType};
use serenity::model::gateway::Ready;
use serenity::model::guild::Member;
use serenity::model::guild::Role;
use serenity::model::id::{GuildId, RoleId, UserId};
use serenity::prelude::*;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::ApplicationCommand(command) = interaction {
            println!("Received command interaction: {:#?}", command);

            let characters = command
                .data
                .options
                .get(0)
                .map(|message| {
                    let text = message
                        .value
                        .as_ref()
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    Characters::parse(text)
                })
                .unwrap_or_default();

            let guild = match command.guild_id {
                None => {
                    eprintln!("command from non-guild");
                    return;
                }
                Some(guild_id) => guild_id,
            };
            let mut member = command.member.clone().unwrap();

            let cmd = command.data.name.as_str();
            let kind = match cmd {
                "main" => CharKind::Main,
                "secondary" => CharKind::Secondary,
                _ => {
                    eprintln!("command not found: {cmd}");
                    return;
                }
            };

            let result = if let Err(err) =
                handler_for_kind(&ctx, &guild, &mut member, characters, kind).await
            {
                eprintln!("failed to run application command: {}", &err);
                err.to_string()
            } else {
                "Roles successfully updated".to_string()
            };

            if let Err(err) = command
                .create_interaction_response(&ctx.http, |response| {
                    response
                        .kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|message| message.content(result))
                })
                .await
            {
                eprintln!("unable to respond: {}", err);
            }
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);

        let guild_id = GuildId(
            env::var("GUILD_ID")
                .expect("Expected GUILD_ID in environment")
                .parse()
                .expect("GUILD_ID must be an integer"),
        );

        if let Err(err) = GuildId::set_application_commands(&guild_id, &ctx.http, |commands| {
            commands
                .create_application_command(|command| {
                    role_creation_command(command, "main", "Set your mains")
                })
                .create_application_command(|command| {
                    role_creation_command(command, "secondary", "Set your secondaries")
                })
        })
        .await
        {
            println!("failed to create application commands: {:#?}", err);
        }
    }
}

fn role_creation_command<'a>(
    command: &'a mut CreateApplicationCommand,
    name: &str,
    descr: &str,
) -> &'a mut CreateApplicationCommand {
    command
        .name(name)
        .description(descr)
        .create_option(|option| {
            option
                .name("characters")
                .description("Characters separated by comma")
                .kind(serenity::model::prelude::command::CommandOptionType::String)
                .required(true)
        })
}

#[tokio::main]
async fn main() {
    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    let mut client = Client::builder(
        token,
        GatewayIntents::non_privileged() | GatewayIntents::GUILDS | GatewayIntents::MESSAGE_CONTENT,
    )
    .event_handler(Handler)
    .await
    .expect("Error creating client");

    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }
}

async fn handler_for_kind(
    ctx: &Context,
    guild: &GuildId,
    member: &mut Member,
    characters: Characters,
    kind: CharKind,
) -> serenity::Result<()> {
    kind.clear(ctx, &guild, member).await?;
    kind.assign_characters(ctx, &guild, member, &characters)
        .await?;

    println!(
        "successfully assigned {:?} to user {}",
        &characters, &member.user.name
    );

    Ok(())
}

enum CharKind {
    Main,
    Secondary,
}

impl CharKind {
    fn postfix(&self) -> &str {
        match self {
            Self::Main => " main",
            Self::Secondary => " secondary",
        }
    }

    fn colour(&self) -> u64 {
        // https://gist.github.com/thomasbnt/b6f455e2c7d743b796917fa3c205f812
        match self {
            Self::Main => 15844367,      // GOLD
            Self::Secondary => 12745742, // DARK_GOLD
        }
    }

    async fn clear(
        &self,
        ctx: &Context,
        guild: &GuildId,
        member: &mut Member,
    ) -> serenity::Result<()> {
        let members = guild.members(ctx, None, None).await?;
        let roles = guild.roles(ctx).await?;

        for role_id in member.roles.clone() {
            let role = roles
                .get(&role_id)
                .ok_or(serenity::Error::Other("corrupt role instance"))?;

            if is_character_role(role, self) {
                println!("trying to remove role from user: {}", &role.name);
                member.remove_role(ctx, role_id).await?;

                if is_role_empty(&members, member.user.id, &role_id) {
                    println!("trying to remove role from guild: {}", &role.name);
                    guild.delete_role(ctx, role_id).await?;
                }
            }
        }

        Ok(())
    }

    async fn assign_characters(
        &self,
        ctx: &Context,
        guild: &GuildId,
        member: &mut Member,
        characters: &Characters,
    ) -> serenity::Result<()> {
        let roles = guild.roles(ctx).await?;

        for char_name in characters.0.iter() {
            let role_name_for_char = format!("{char_name} {}", self.postfix());

            if let Some(role_id) = roles.values().find(|role| role.name == role_name_for_char) {
                println!(
                    "adding existing role {role_name_for_char} to {}",
                    &member.user.name
                );
                member.add_role(ctx, role_id).await?;
            } else {
                println!("creating new role {role_name_for_char}");
                let role_id = new_role(ctx, guild, &role_name_for_char, self.colour()).await?;
                println!(
                    "adding new role {role_name_for_char} to {}",
                    &member.user.name
                );
                member.add_role(ctx, role_id).await?;
            }
        }

        Ok(())
    }
}

fn is_character_role(role: &Role, kind: &CharKind) -> bool {
    let yes = role.name.ends_with(kind.postfix());
    println!("does {} end with {}? {}", role.name, kind.postfix(), yes);
    yes
}

fn is_role_empty(members: &[Member], self_: UserId, role: &RoleId) -> bool {
    members
        .iter()
        .all(|m| m.user.id == self_ || !m.roles.contains(role))
}

async fn new_role(
    ctx: &Context,
    guild: &GuildId,
    role: &str,
    color: u64,
) -> serenity::Result<Role> {
    guild.create_role(ctx, |r| r.colour(color).name(role)).await
}

#[derive(Debug, Clone, Default)]
struct Characters(Vec<String>);

impl Characters {
    fn parse(text: &str) -> Self {
        let vec = text
            .split(',')
            .map(str::trim)
            .filter(|str| str.len() > 1)
            .map(capitalize_words)
            .map(find_alias)
            .collect();

        fn capitalize_words(str: &str) -> String {
            let mut previous = ' ';
            str.chars()
                .map(|mut c| {
                    if previous == ' ' {
                        c.make_ascii_uppercase();
                    } else {
                        c.make_ascii_lowercase();
                    }
                    previous = c;
                    c
                })
                .collect()
        }

        Characters(vec)
    }
}

fn find_alias(char: String) -> String {
    identify_character(&char).map(String::from).unwrap_or(char)
}

fn identify_character(char: &str) -> Option<&'static str> {
    if char.contains("Game") || char.contains("Watch") {
        return Some("Game & Watch");
    }

    if char.contains("Banjo") || char.contains("Kazooie") {
        return Some("Game & Watch");
    }

    if char.contains("Rosalina") {
        return Some("Rosalina & Luma");
    }

    if (char.contains("Pyra") && char.contains("Mythra")) || char.contains("Aegis") {
        return Some("Aegis");
    }

    match char {
        "G&w" | "G & W" => Some("Game & Watch"),
        "Dk" => Some("Donkey Kong"),
        _ => None,
    }
}
