#ifndef __UTILS_HPP__
#define __UTILS_HPP__

#include <Arduino.h>
#include <driver/rtc_io.h>

void lightSleep(gpio_num_t gpio, int level) {
  Serial.println("Going into light sleep...");
  Serial.flush();

  rtc_gpio_hold_en(gpio);
  esp_sleep_enable_ext0_wakeup(gpio, level);
  esp_light_sleep_start();

  Serial.println("Waked up from light sleep...");

  delay(100);
}

float analogReadAvg(int pin, int c = 10) {
  float v = 0;
  for (int i = 0; i < c; i++) {
    v += analogRead(pin);
  }

  return v / c;
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
float readBatteryVoltage(int pin) {
  float val = analogReadAvg(pin);
  float voltage = val * READ_OFFSET * (V_REF / MAX_ADC) * ((R1 + R2) / R2);

  return voltage + VOLTAGE_OFFSET;
}

#endif