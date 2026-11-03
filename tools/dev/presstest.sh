#!/bin/sh
# Simulates a short press on the power button (evdev injection), with
# an RTC alarm to wake up, then a second press injected on wake-up
# so that Signet treats it as a real wake-up. Log: /tmp/presstest.log
WAKE=${1:-120}
exec > /tmp/presstest.log 2>&1
press() {  # type=1 code=116 (KEY_POWER) value=$1 then SYN_REPORT
  printf '\000\000\000\000\000\000\000\000\001\000\164\000'"$1"'\000\000\000' > /dev/input/event0
  printf '\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000' > /dev/input/event0
}
echo "debut uptime $(cut -d" " -f1 /proc/uptime)"
echo 0 > /sys/class/rtc/rtc0/wakealarm; echo "+$WAKE" > /sys/class/rtc/rtc0/wakealarm
echo "alarme +$WAKE"
press '\001'; sleep 0.2; press '\000'
echo "appui court injecté"
LAST=$(cut -d. -f1 /proc/uptime)
while :; do
  sleep 1
  NOW=$(cut -d. -f1 /proc/uptime)
  if [ $((NOW - LAST)) -gt 8 ]; then
    echo "saut d'uptime $LAST -> $NOW : réveil détecté"
    sleep 1; press '\001'; sleep 0.2; press '\000'
    echo "appui de réveil injecté"
    exit 0
  fi
  LAST=$NOW
  [ $((NOW)) -gt $((LAST + WAKE + 60)) ] && { echo "timeout"; exit 1; }
done
