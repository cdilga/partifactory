# Opening links from Telegram

A real-phone check of passkey sign-in in Telegram's in-app browser is a nonblocking human check. Run the five-minute procedure in [the device checklist](../../tests/manual/real-device-pass.md); record the operating system, browser versions and result, without names or URLs containing capabilities.

If Telegram's in-app browser cannot complete sign-in, apply both decided fixes:

1. In Telegram's link or browser settings, choose the phone's normal browser for links. Settings names vary by Telegram version; the message's menu can also open a link in the external browser.
2. Have the chat agent's messages include the hint **“Open in browser”** beside the part's own host link, such as `https://notes-part.example.com/`.

The chat agent supplies the hint in its own messages. Partifactory does not send Telegram messages. Do not introduce person-bound public links to work around webview limitations. A failure found on a real phone is filed as a bug linked to the human-checks bead and does not block a release.
