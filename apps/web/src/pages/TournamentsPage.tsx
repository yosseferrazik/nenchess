export function TournamentsPage() {
  return (
    <section className="tournaments">
      <h2>Torneos</h2>
      <div className="panel">
        <p>
          Módulo planificado para la <strong>etapa 05</strong> (crecimiento de plataforma). El esqueleto ya reserva
          su lugar: <code>services/api/src/modules/tournaments</code> y las tablas <code>tournaments</code> /{' '}
          <code>tournament_participants</code> que se añadirán con una migración nueva.
        </p>
        <p className="muted small">
          Decisión de diseño ya fijada: el mismo motor de partidas autoritativo se reutiliza para los enfrentamientos
          de un torneo, de modo que relojes, validación y desempates no se duplican.
        </p>
      </div>
    </section>
  );
}
