from __future__ import annotations

from maubot import Plugin, MessageEvent
from maubot.handlers import command
from mautrix.util.config import BaseProxyConfig, ConfigUpdateHelper


class Config(BaseProxyConfig):
    def do_update(self, helper: ConfigUpdateHelper) -> None:
        helper.copy("admin_url")
        helper.copy("bot_api_secret")
        helper.copy("ops_room_id")


class InviteBot(Plugin):

    @classmethod
    def get_config_class(cls) -> type[Config]:
        return Config

    async def start(self) -> None:
        self.config.load_and_update()

    @command.new(
        "invite",
        help="Invite a user to Matrix by email: !invite user@example.com",
    )
    @command.argument("email", pass_raw=True, required=True)
    async def invite_command(self, evt: MessageEvent, email: str) -> None:
        ops_room = self.config["ops_room_id"]
        if ops_room and evt.room_id != ops_room:
            return

        email = email.strip().lower()
        if not email or "@" not in email:
            await evt.reply("Usage: `!invite user@example.com`")
            return

        admin_url = self.config["admin_url"].rstrip("/")
        secret = self.config["bot_api_secret"]

        try:
            resp = await self.http.post(
                f"{admin_url}/api/v1/invites",
                headers={
                    "Authorization": f"Bearer {secret}",
                    "Content-Type": "application/json",
                },
                json={
                    "email": email,
                    "invited_by": str(evt.sender),
                },
            )
        except Exception as exc:
            self.log.exception("Failed to reach admin service")
            await evt.reply(f"Could not reach the admin service: {exc}")
            return

        try:
            data = await resp.json(content_type=None)
        except Exception:
            await evt.reply(f"Admin service returned an unexpected response (HTTP {resp.status}).")
            return

        message = data.get("message", "No message returned.")

        if resp.status == 201 and data.get("ok"):
            await evt.reply(f"Invite sent to **{email}**. They will receive an email with a link to set their password.")
        elif resp.status == 422:
            await evt.reply(f"Could not invite **{email}**: {message}")
        elif resp.status == 401:
            self.log.error("Bot API secret rejected by admin service — check BOT_API_SECRET config")
            await evt.reply("Bot is misconfigured (auth error). Contact a server admin.")
        elif resp.status == 502:
            await evt.reply(f"The admin service could not reach Keycloak: {message}")
        else:
            await evt.reply(f"Unexpected response (HTTP {resp.status}): {message}")
