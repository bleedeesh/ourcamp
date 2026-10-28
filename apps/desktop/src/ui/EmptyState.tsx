import { Campfire } from "@ourcamp/ui/Campfire.tsx";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { useCampsController } from "./camps-context.tsx";

/** First run: no camps yet (mockup screen 3). */
export function EmptyState() {
  const camps = useCampsController();
  return (
    <section className="empty" aria-labelledby="empty-title">
      <Campfire size={72} />
      <h2 id="empty-title" className="empty__title">
        Пока ни одного сервера
      </h2>
      <p className="empty__text">Создайте свой и позовите друзей или войдите по ссылке-приглашению.</p>
      <button type="button" className="primary-button" onClick={() => camps.openJoin()}>
        <Icon name="link" size={18} />
        Войти по ссылке
      </button>
      {/* Hosting from the app comes with the server window (roadmap M3, step 6). */}
      <button type="button" className="wide-button" aria-disabled="true" title="Скоро: окно сервера">
        <Icon name="plus" size={18} />
        Создать свой сервер
      </button>
    </section>
  );
}
