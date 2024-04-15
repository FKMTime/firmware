#ifndef __DEFINES_H__
#define __DEFINES_H__

#define LCD_ADDR 0x27
#define LCD_SIZE_X 16
#define LCD_SIZE_Y 2

#define SLEEP_TIME 600000 // 10mins
#define BATTERY_READ_INTERVAL 30000 // 30s

#define NAME_PREFIX "FkmTimer"
#define WIFI_PASSWORD "FkmTimer"

#define INSPECTION_TIME 15000 // 15s
#define INSPECTION_PLUS_TWO_PENALTY 15000 // 15s to dnf penalty
#define INSPECTION_DNF_PENALTY 17000 // from 17s upwards

#define DELEGAT_BUTTON_HOLD_TIME 3000 // 3s (in 1s increments)
#define DNF_BUTTON_HOLD_TIME 1000 // on penalty button (TIME TO HOLD PNALTY TO INPUT DNF)
#define RESET_COMPETITOR_HOLD_TIME 5000 // on submit button (reset competitor and time)
#define RESET_WIFI_HOLD_TIME 15000 // on submit button

#endif