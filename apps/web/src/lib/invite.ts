//! Invite links for a game.
//!
//! A link is simply a deep link to `/games/:gameId`; the router already handles
//! it, and a visitor without a session is bounced to `/auth` first and can then
//! open the game they were invited to.

export function inviteLink(gameId: string): string {
  return `${window.location.origin}/games/${gameId}`;
}

/** Copy the invite link. Returns whether it made it to the clipboard. */
export async function copyInviteLink(gameId: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(inviteLink(gameId));
    return true;
  } catch {
    return false;
  }
}
