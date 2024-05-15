#ifndef __UTILS_HPP__
#define __UTILS_HPP__

#include <Arduino.h>
#include <driver/rtc_io.h>
#include "globals.hpp"
#include "version.h"

float batteryVoltageOffset = 0;

void lightSleep(gpio_num_t gpio, int level) {
  Logger.println("Going into light sleep...");
  Serial.flush();
  Logger.loop(true);
  webSocket.loop();
  webSocket.disconnect();

  rtc_gpio_hold_en(gpio);
  esp_sleep_enable_ext0_wakeup(gpio, level);
  esp_light_sleep_start();

  Logger.println("Waked up from light sleep...");
}

uint16_t analogReadMax(int pin, int c = 10, int delayMs = 5) {
  uint16_t v = 0;
  for (int i = 0; i < c; i++) {
    v = max(analogRead(pin), v);
    delay(delayMs);
  }

  return v;
}

#define MIN_VOLTAGE 3.0 // to change (measure min voltage of cell when esp is turning on)
#define MAX_VOLTAGE 4.2
float voltageToPercentage(float voltage) {
  voltage = constrain(voltage, MIN_VOLTAGE, MAX_VOLTAGE);

  return ((voltage - MIN_VOLTAGE) / (MAX_VOLTAGE - MIN_VOLTAGE)) * 100;
}

#define V_REF 3.3
#define READ_OFFSET 1.0 // to change
#define MAX_ADC 4095.0 // max adc - 1
#define R1 10000
#define R2 10000
float readBatteryVoltage(int pin, int delayMs = 5, bool offset = true) {
  float val = analogReadMax(pin, 10, delayMs);
  float voltage = val * READ_OFFSET * (V_REF / MAX_ADC) * ((R1 + R2) / R2);

  return voltage + (offset ? batteryVoltageOffset : 0);
}

void sendBatteryStats(float level, float voltage) {
  JsonDocument doc;
  doc["battery"]["esp_id"] = getEspId();
  doc["battery"]["level"] = level;
  doc["battery"]["voltage"] = voltage;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
}

#define ADD_DEVICE_FIRMWARE_TYPE "STATION"
void sendAddDevice() {
  JsonDocument doc;
  doc["add"]["esp_id"] = getEspId();
  doc["add"]["firmware"] = ADD_DEVICE_FIRMWARE_TYPE;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
}

unsigned long getEpoch() {
  struct tm timeinfo;
  if (!getLocalTime(&timeinfo)) {
    Logger.println("Failed to obtain time");
  }
  time_t epoch;
  time(&epoch);

  return epoch;
}

void clearDisplay(uint8_t filler = 255) {
  digitalWrite(DIS_STCP, LOW);

  for(int i = 0; i < DIS_LENGTH; i++){
    shiftOut(DIS_DS, DIS_SHCP, LSBFIRST, filler);
  }

  digitalWrite(DIS_STCP, HIGH);
}

int decDigits[10] = {215,132,203,206,156,94,95,196,223,222};
int dotMod = 32;
void displayStr(String str) {
  digitalWrite(DIS_STCP, LOW);

  int pos = 0;
  for(int i = 0; i < DIS_LENGTH - str.length(); i++) {
    shiftOut(DIS_DS, DIS_SHCP, LSBFIRST, 255);
    pos++;
  }

  for (int i = 0; i < str.length(); i++) {
    bool showDot = pos == 0 || pos == 2; // show '.' and ':'

    int digit = str[i] - '0';
    shiftOut(DIS_DS, DIS_SHCP, LSBFIRST, ~decDigits[digit] ^ (showDot ? dotMod : 0));
    pos++;
  }

  digitalWrite(DIS_STCP, HIGH);
}

String displayTime(uint8_t m, uint8_t s, uint16_t ms, bool special = true) {
  String tmp = "";
  if (m > 0) {
    tmp += m;
    if(special) tmp += ":";

    char sBuff[6];
    sprintf(sBuff, "%02d", s);
    tmp += String(sBuff);
  } else {
    tmp += s;
  }

  char msBuff[6];
  sprintf(msBuff, "%03d", ms);

  if(special) tmp += ".";
  tmp += String(msBuff);
  return tmp;
}

#endif