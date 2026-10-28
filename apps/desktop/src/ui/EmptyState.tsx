import { Campfire } from "./Campfire.tsx";
import { Icon } from "./icons.tsx";
import { useCampsController } from "./camps-context.tsx";

/** First run: no camps yet (mockup screen 3). */
export function EmptyState() {
  const camps = useCampsController();
  return (
    <section className="empty" aria-labelledby="empty-title">
      <Campfire size={72} />
      <h2 id="empty-title" className="empty__title">
        Пока ни одного лагеря
      </h2>
      <p className="empty__text">Создайте свой и позовите друзей или войдите по ссылке-приглашению.</p>
      <button type="button" className="primary-button" onClick={() => camps.openJoin()}>
        <Icon name="link" size={18} />
        Войти по ссылке
      </button>
      {/* Hosting from the app comes with the server window (roadmap M3, step 6). */}
      <button type="button" className="wide-button" aria-disabled="true" title="Скоро: окно сервера лагеря">
        <Icon name="plus" size={18} />
        Создать свой лагерь
      </button>
    </section>
  );
}
