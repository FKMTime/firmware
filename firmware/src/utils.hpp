#ifndef __UTILS_HPP__
#define __UTILS_HPP__

#include <Arduino.h>

extern float batteryVoltageOffset;

void lightSleep(gpio_num_t gpio, int level);
uint16_t analogReadMax(int pin, int c = 10, int delayMs = 5);

// to change (measure min voltage of cell when esp is turning on)
#define MIN_VOLTAGE 3.0
#define MAX_VOLTAGE 4.2
float voltageToPercentage(float voltage);

#define V_REF 3.3
#define READ_OFFSET 1.0 // to change
#define MAX_ADC 4095.0  // max adc - 1
#define R1 10000.0
#define R2 10000.0
float readBatteryVoltage(int pin, int delayMs = 5, bool offset = true);
void sendBatteryStats(float level, float voltage);

#define ADD_DEVICE_FIRMWARE_TYPE "STATION"
void sendAddDevice();

extern unsigned long epochBase;
unsigned long getEpoch();

void displayStr(String str);
void clearDisplay(uint8_t filler = 255);
String displayTime(uint8_t m, uint8_t s, uint16_t ms, bool special = true);

#endif
