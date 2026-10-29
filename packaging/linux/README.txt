OurCamp Server для Linux
=======================

Сервер для голосового клиента OurCamp: друзья заходят по ссылке ourcamp://…
Внутри архива всё нужное: oc-server (собран статически, работает на любом
дистрибутиве x86_64 / arm64) и livekit-server (голос).

Установка (нужен systemd)
-------------------------
    tar xzf ourcamp-server-*-linux-*.tar.gz
    cd ourcamp-server-*/
    sudo ./install.sh --host ВАШ-ДОМЕН-ИЛИ-IP

Ссылка для друзей:
    sudo ourcamp status

Откройте порты в файрволе (и у хостинга, если у него свой):
    TCP 7780   сервер
    TCP 7881   голос (запасной путь)
    UDP 7882   голос

Что где:
    /opt/ourcamp/                   программы
    /etc/ourcamp/server.env         настройки
    /var/lib/ourcamp/               база, сертификат сервера — делайте копию:
                                    без неё старые ссылки перестанут работать
    sudo journalctl -u ourcamp-server -f    журнал

Обновление: распакуйте новый архив и снова sudo ./install.sh — настройки
и данные остаются.

Управление (как в окне сервера)
-------------------------------
    sudo ourcamp status
    sudo ourcamp role Лена admin           участник / модератор / админ
    sudo ourcamp rename Вечерний кооп
    sudo ourcamp channel add Катка         голосовой канал (--text — чат)
    sudo ourcamp kick Боб
    sudo ourcamp open                      пускать всех, у кого есть адрес
    sudo ourcamp invite-only               только по приглашению
Все команды: ourcamp --help

Домен с сертификатом Let's Encrypt
----------------------------------
Тогда ссылка без отпечатка: ourcamp://ourcamp.kz

1. DNS: запись A для домена → IP сервера.
2. Сертификат (certbot сам займёт порт 80 на минуту):
       sudo certbot certonly --standalone -d ourcamp.kz \
         --deploy-hook /opt/ourcamp/certbot-deploy-hook.sh
   Хук кладёт файлы в /etc/ourcamp/tls/ и делает это при каждом продлении —
   сервер подхватывает новый сертификат сам, без перезапуска.
3. В /etc/ourcamp/server.env:
       OURCAMP_ARGS=--host ourcamp.kz --tls-cert /etc/ourcamp/tls/fullchain.pem --tls-key /etc/ourcamp/tls/privkey.pem
   и sudo systemctl restart ourcamp-server

Старые ссылки с отпечатком (?fp=…) продолжают работать.

Удаление
--------
    sudo systemctl disable --now ourcamp-server
    sudo rm /etc/systemd/system/ourcamp-server.service /usr/local/bin/ourcamp
    sudo rm -r /opt/ourcamp /etc/ourcamp
    sudo rm -r /var/lib/ourcamp            это сам сервер: каналы, люди, ссылки
    sudo userdel ourcamp
