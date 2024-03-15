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

#endif