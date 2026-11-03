#!/bin/sh
# Sleep experiment, to run ON the Kobo (detached): turns Wi-Fi off,
# suspends, records the timings and dmesg, then brings Wi-Fi back up so the
# result can be read back over SSH. Usage: setsid /tmp/sleeptest.sh &
LOG=/tmp/sleeptest.log
exec > $LOG 2>&1
echo "=== debut uptime $(cut -d" " -f1 /proc/uptime) $(date)"
killall wpa_supplicant udhcpc 2>/dev/null; sleep 1
ifconfig eth0 down 2>/dev/null
rmmod 8189fs 2>/dev/null; rmmod sdio_wifi_pwr 2>/dev/null
echo "modules: $(cut -d" " -f1 /proc/modules | tr "\n" " ")"
echo "wakealarm=$(cat /sys/class/rtc/rtc0/wakealarm) wakeup_count=$(cat /sys/power/wakeup_count)"
echo 0 > /sys/class/backlight/mxc_msp430.0/brightness 2>/dev/null
# Automatic wake-up by the real-time clock: the experiment depends on no one.
WAKE=${1:-90}
echo 0 > /sys/class/rtc/rtc0/wakealarm 2>/dev/null
echo "+$WAKE" > /sys/class/rtc/rtc0/wakealarm && echo "alarme RTC dans $WAKE s (wakealarm=$(cat /sys/class/rtc/rtc0/wakealarm))" 
dmesg -c > /dev/null
echo 1 > /sys/power/state-extended; sleep 2; sync
S=$(cut -d" " -f1 /proc/uptime)
echo mem > /sys/power/state; R=$?
E=$(cut -d" " -f1 /proc/uptime)
echo "ret=$R entry=$S exit=$E"
echo 0 > /sys/power/state-extended
echo "=== dmesg"; dmesg | grep -v "PMU\|calc_\|ricoh61x_battery"
echo "=== wifi retour"
insmod /drivers/mx6ull-ntx/wifi/sdio_wifi_pwr.ko; insmod /drivers/mx6ull-ntx/wifi/8189fs.ko; sleep 2
ifconfig eth0 up
wpa_supplicant -D wext -s -i eth0 -O /var/run/wpa_supplicant -c /tmp/signet-wpa.conf -B
udhcpc -S -i eth0 -s /etc/udhcpc.d/default.script -t 15 -T 10 -A 3 -b
sleep 8; ifconfig eth0 | grep inet
cp $LOG /mnt/onboard/.signet/sleeptest.log 2>/dev/null; sync
echo "=== fin"
