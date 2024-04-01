#ifndef __UTILS_HPP__
#define __UTILS_HPP__

#include <Arduino.h>
#include <driver/rtc_io.h>
#include "globals.hpp"
#include "version.h"

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
#define MAX_VOLTAGE 4.1
float voltageToPercentage(float voltage) {
  voltage = constrain(voltage, MIN_VOLTAGE, MAX_VOLTAGE);

  return ((voltage - MIN_VOLTAGE) / (MAX_VOLTAGE - MIN_VOLTAGE)) * 100;
}

#define V_REF 3.3
#define READ_OFFSET 1.08 // to change
#define VOLTAGE_OFFSET 0.1 // to change
#define MAX_ADC 4095.0 // max adc - 1
#define R1 10000
#define R2 10000
float readBatteryVoltage(int pin, int delayMs = 5, bool offset = true) {
  float val = analogReadMax(pin, 10, delayMs);
  float voltage = val * READ_OFFSET * (V_REF / MAX_ADC) * ((R1 + R2) / R2);

  return voltage + (offset ? VOLTAGE_OFFSET : 0);
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

void sendAddDevice() {
  JsonDocument doc;
  doc["add"]["esp_id"] = getEspId();
  doc["add"]["firmware"] = FIRMWARE_TYPE;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
}

#endif