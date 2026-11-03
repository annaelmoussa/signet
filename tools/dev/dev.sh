#!/bin/sh
# Development hook, lives on the partition (.signet/dev.sh): run by
# signetd after Nickel stops, and relaunched by Signet as long as there is
# no IP address. Log: /tmp/signetd.log and .signet/dev.log.
LOG=/tmp/signetd.log
# Log in /tmp; copied to the partition by onboard_write (it is
# read-only at rest: we remount it writable just long enough to write, sync, and switch back).
log() { echo "[dev.sh $(cut -d" " -f1 /proc/uptime)] $*" >> $LOG; }
onboard_write() {
  [ -d /mnt/onboard/.signet ] || return 0
  RO=0; grep -q ' /mnt/onboard [^ ]* [^ ]*\bro\b' /proc/mounts && RO=1
  [ $RO = 1 ] && mount -o remount,rw /mnt/onboard 2>/dev/null
  grep '^\[dev.sh' $LOG > /mnt/onboard/.signet/dev.log 2>/dev/null
  [ -n "$1" ] && echo -n "$1" > /mnt/onboard/.signet/ip.txt 2>/dev/null
  sync
  [ $RO = 1 ] && mount -o remount,ro /mnt/onboard 2>/dev/null
}
log "début"
CONF=/mnt/onboard/.signet/wifi.conf
[ -f "$CONF" ] || { log "pas de wifi.conf"; exit 0; }
SSID=$(sed -n 's/^ssid=//p' "$CONF" | head -1)
PSK=$(sed -n 's/^psk=//p' "$CONF" | head -1)
PLAT=mx6ull-ntx
grep -q imx6sll /proc/device-tree/compatible 2>/dev/null && PLAT=mx6sll-ntx
# One instance at a time.
[ -e /tmp/dev.sh.lock ] && kill -0 $(cat /tmp/dev.sh.lock) 2>/dev/null && { log "déjà en cours"; exit 0; }
echo $$ > /tmp/dev.sh.lock
printf 'ctrl_interface=/var/run/wpa_supplicant\nnetwork={\n\tssid="%s"\n\tpsk="%s"\n}\n' "$SSID" "$PSK" > /tmp/signet-wpa.conf
mkdir -p /var/run/wpa_supplicant /var/empty /dev/pts; addgroup --gid 999 sshd 2>/dev/null
mount -t devpts devpts /dev/pts 2>/dev/null
# sshd in the background: at startup it waits for the kernel's random number
# generator to be ready (~45 s without a hardware RNG); Wi-Fi need not wait for that.
# SSH server: Dropbear embedded in Signet (the fallback firmware has no
# sshd). Host keys generated on first boot in /etc/dropbear;
# authorized key copied to /.ssh (root's HOME is "/").
DB=/usr/local/signet/dropbearmulti
if ! { [ -e /tmp/sshd-signet.pid ] && kill -0 $(cat /tmp/sshd-signet.pid) 2>/dev/null; }; then
  if [ -x "$DB" ]; then
    mkdir -p /etc/dropbear /.ssh && chmod 700 /.ssh
    cp /usr/local/signet/authorized_keys /.ssh/authorized_keys && chmod 600 /.ssh/authorized_keys
    # In the background: the first key generation waits for kernel entropy (~50 s),
    # Wi-Fi need not wait for that.
    ( [ -s /etc/dropbear/dropbear_ed25519_host_key ] || $DB dropbearkey -t ed25519 -f /etc/dropbear/dropbear_ed25519_host_key >>$LOG 2>&1
      $DB dropbear -R -s -p 22 -P /tmp/sshd-signet.pid >>$LOG 2>&1 && log "dropbear démarré" || log "dropbear : échec (code $?)" ) &
  elif [ -x /usr/sbin/sshd ]; then
    ( /usr/sbin/sshd -f /usr/local/signet/sshd_config >>$LOG 2>&1 && log "sshd démarré" || log "sshd : échec (code $?)" ) &
  else
    log "aucun serveur SSH disponible"
  fi
fi
# Let Signet initialize the screen before powering the Wi-Fi chip (15 s at most).
for i in $(seq 1 15); do [ -e /tmp/signet-ready ] && break; sleep 1; done
log "Signet prêt : $([ -e /tmp/signet-ready ] && echo oui || echo non) ; heure $(date -u '+%Y-%m-%d %H:%M:%S') UTC"
# (after the wait: the partition is not remounted while Signet starts)
# Wi-Fi log from the previous boot kept (dev.log.1), to understand a
# boot that ended up without a network.
if [ ! -e /tmp/dev.sh.rotated ] && [ -s /mnt/onboard/.signet/dev.log ]; then
  touch /tmp/dev.sh.rotated
  RO=0; grep -q ' /mnt/onboard [^ ]* [^ ]*\bro\b' /proc/mounts && RO=1
  [ $RO = 1 ] && mount -o remount,rw /mnt/onboard 2>/dev/null
  mv /mnt/onboard/.signet/dev.log /mnt/onboard/.signet/dev.log.1 2>/dev/null
  sync
  [ $RO = 1 ] && mount -o remount,ro /mnt/onboard 2>/dev/null
fi
for attempt in 1 2 3; do
  log "tentative $attempt"
  killall wpa_supplicant udhcpc 2>/dev/null; sleep 1
  ifconfig eth0 down 2>/dev/null
  rmmod 8189fs 2>/dev/null; rmmod sdio_wifi_pwr 2>/dev/null; sleep 1
  insmod /drivers/$PLAT/wifi/sdio_wifi_pwr.ko 2>>$LOG || log "insmod sdio_wifi_pwr : échec"
  insmod /drivers/$PLAT/wifi/8189fs.ko 2>>$LOG || log "insmod 8189fs : échec"
  for i in 1 2 3 4 5 6 7 8; do [ -e /sys/class/net/eth0 ] && break; sleep 1; done
  [ -e /sys/class/net/eth0 ] || { log "pas d'interface eth0"; continue; }
  ifconfig eth0 up || log "ifconfig up : échec"
  wpa_supplicant -D wext -s -i eth0 -O /var/run/wpa_supplicant -c /tmp/signet-wpa.conf -B && log "wpa_supplicant lancé ($SSID)" || log "wpa_supplicant : échec"
  udhcpc -S -i eth0 -s /etc/udhcpc.d/default.script -t 15 -T 10 -A 3 -b >/dev/null 2>&1
  for i in $(seq 1 30); do
    IP=$(ifconfig eth0 2>/dev/null | sed -n 's/.*inet addr:\([0-9.]*\).*/\1/p')
    if [ -n "$IP" ]; then
      echo -n "$IP" > /tmp/signet-ip
      log "eth0 = $IP"; onboard_write "$IP"; rm -f /tmp/dev.sh.lock; exit 0
    fi
    sleep 1
  done
  log "pas d'IP (état wpa : $(wpa_cli -p /var/run/wpa_supplicant -i eth0 status 2>/dev/null | sed -n 's/^wpa_state=//p'))"
done
rm -f /tmp/dev.sh.lock
log "abandon après 3 tentatives"
onboard_write ""
