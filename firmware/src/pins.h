#ifndef __PINS_H__
#define __PINS_H__

#include <Arduino.h>

#define BUTTON1 33 // submit
#define BUTTON2 32 // penalty
#define BUTTON3 27 // inspection start
#define BUTTON4 26 // delegate
#define SLEEP_WAKE_BUTTON (gpio_num_t)STACKMAT_JACK

#define BAT_ADC 34
#define STACKMAT_JACK 4

// DEFAULT VSPI PINOUT
#define RFID_CS 5
#define RFID_SCK 18
#define RFID_MISO 19
#define RFID_MOSI 23

// DEFAULT I2C PINOUT
#define LCD_SDA 21
#define LCD_SCL 22

// DISPLAY SHIFTER
#define DIS_DS 16
#define DIS_STCP 17
#define DIS_SHCP 12

#endif