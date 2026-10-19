//! Thin re-export of the contract generated from `crates/protocol`.
//!
//! Nothing here is hand-written except helpers; the types themselves come
//! from the Rust source of truth (`cargo test -p protocol` regenerates them
//! into `src/lib/generated`).

export type { AppliedMoveDto } from './generated/AppliedMoveDto';
export type { AuthResponse } from './generated/AuthResponse';
export type { ClockDto } from './generated/ClockDto';
export type { ClientMessage } from './generated/ClientMessage';
export type { ColorDto } from './generated/ColorDto';
export type { CreateGameRequest } from './generated/CreateGameRequest';
export type { ErrorCode } from './generated/ErrorCode';
export type { ErrorDto } from './generated/ErrorDto';
export type { GameHistoryEntryDto } from './generated/GameHistoryEntryDto';
export type { GameHistoryResponse } from './generated/GameHistoryResponse';
export type { GameOutcomeDto } from './generated/GameOutcomeDto';
export type { GameResultDto } from './generated/GameResultDto';
export type { GameStatusDto } from './generated/GameStatusDto';
export type { GameView } from './generated/GameView';
export type { GamesResponse } from './generated/GamesResponse';
export type { LoginRequest } from './generated/LoginRequest';
export type { MatchmakingRequest } from './generated/MatchmakingRequest';
export type { MatchmakingStatusDto } from './generated/MatchmakingStatusDto';
export type { MoveDto } from './generated/MoveDto';
export type { MoveInputDto } from './generated/MoveInputDto';
export type { PieceKindDto } from './generated/PieceKindDto';
export type { PlayerDto } from './generated/PlayerDto';
export type { RegisterRequest } from './generated/RegisterRequest';
export type { ResultReasonDto } from './generated/ResultReasonDto';
export type { ServerMessage } from './generated/ServerMessage';
export type { UserDto } from './generated/UserDto';
export type { ProfileDto } from './generated/ProfileDto';
export type { RatingDto } from './generated/RatingDto';
export type { CreateTournamentRequest } from './generated/CreateTournamentRequest';
export type { PairingDto } from './generated/PairingDto';
export type { ParticipantDto } from './generated/ParticipantDto';
export type { RoundDto } from './generated/RoundDto';
export type { TournamentDto } from './generated/TournamentDto';
export type { TournamentStatusDto } from './generated/TournamentStatusDto';
export type { TournamentView } from './generated/TournamentView';
export type { TournamentsResponse } from './generated/TournamentsResponse';
