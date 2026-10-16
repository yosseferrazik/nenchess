//! Clock display. The server is the only authority: this component simply
//! interpolates locally from the last snapshot (`white_ms`/`black_ms` at
//! `server_now_ms`) and never sends anything back.

import { useEffect, useState } from 'react';

import type { ClockDto, ColorDto } from '../lib/protocol';

function format(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  const fraction = Math.max(0, ms) % 1000;
  const tenth = Math.floor(fraction / 100);
  if (minutes === 0 && ms < 20_000) {
    return `${seconds}.${tenth}s`;
  }
  return `${minutes}:${String(seconds).padStart(2, '0')}`;
}

export function Clock({
  clock,
  side,
  label,
}: {
  clock: ClockDto | null;
  side: ColorDto;
  label: string;
}) {
  const [now, setNow] = useState(() => Date.now());
  const running = clock !== null && clock.turn === side;

  useEffect(() => {
    if (!running) return;
    const id = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(id);
  }, [running]);

  if (!clock) {
    return <div className="clock idle">{label} —</div>;
  }

  const base = side === 'white' ? clock.white_ms : clock.black_ms;
  const drift = now - clock.server_now_ms;
  const remaining = Math.max(0, running ? base - drift : base);
  const low = running && remaining < 20_000;

  return (
    <div className={`clock ${running ? 'running' : 'idle'} ${low ? 'low' : ''}`.trim()}>
      <span className="clock-label">{label}</span>
      <span className="clock-time">{format(remaining)}</span>
    </div>
  );
}
