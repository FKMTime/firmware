#ifndef __BUTTONS_HPP__
#define __BUTTONS_HPP__

#include "globals.hpp"

#define DELEGAT_BUTTON_HOLD_TIME 3000
#define DNF_BUTTON_HOLD_TIME 1000

inline void buttonsLoop() {
  if (digitalRead(PENALTY_BUTTON_PIN) == LOW) {
    Logger.println("Penalty button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(PENALTY_BUTTON_PIN) == LOW && millis() - pressedTime <= DNF_BUTTON_HOLD_TIME) {
      webSocket.loop(); // to prevent disconnects
      delay(50);
    }

    if(state.timeConfirmed || state.finishedSolveTime <= 0) return;
    if (millis() - pressedTime > DNF_BUTTON_HOLD_TIME) {
      state.timeOffset = state.timeOffset != -1 ? -1 : 0;
      lcdChange();
      lcdLoop();
    } else { 
      state.timeOffset = (state.timeOffset >= 16 || state.timeOffset == -1) ? 0 : state.timeOffset + 2;
      lcdChange();
    }

    while (digitalRead(PENALTY_BUTTON_PIN) == LOW) {
      webSocket.loop(); // to prevent disconnects
      delay(50);
    }
  }

  if (digitalRead(SUBMIT_BUTTON_PIN) == LOW) {
    Logger.println("Submit button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(SUBMIT_BUTTON_PIN) == LOW) {
      delay(50);
    }

    if (millis() - pressedTime > 5000) {
      // TODO: REMOVE THIS
      Logger.println("Resetting wifi settings!");
      WiFiManager wm;
      wm.resetSettings();
      delay(1000);
      ESP.restart();
    } else {
      if (state.finishedSolveTime > 0 && state.solverCardId > 0) {
        state.timeConfirmed = true;
        lcdChange();
      }
    }
  }

  if (digitalRead(DELEGATE_BUTTON_PIN) == HIGH && state.finishedSolveTime > 0) {
    Logger.println("Delegat button pressed!");
    unsigned long pressedTime = millis();

    lcd.clear();
    while (digitalRead(DELEGATE_BUTTON_PIN) == HIGH && millis() - pressedTime <= DELEGAT_BUTTON_HOLD_TIME) {
      webSocket.loop(); // to prevent disconnects
      delay(100);

      lcd.setCursor(0, 0);
      lcd.printf("Delegat");
      lcd.setCursor(0, 1);
      lcd.printf("Za %lu sekund!", ((DELEGAT_BUTTON_HOLD_TIME + 1000) - (millis() - pressedTime)) / 1000);
    }

    lcdChange();

    if(millis() - pressedTime > DELEGAT_BUTTON_HOLD_TIME) {
      Logger.printf("Wzywanie rozpoczete!");
      lcd.clear();
      lcd.setCursor(0, 0);
      lcd.printf("Delegat wezwany");
      lcd.setCursor(0, 1);
      lcd.printf("Pusc przycisk");

      sendSolve(webSocket, true);
    }

    while (digitalRead(DELEGATE_BUTTON_PIN) == HIGH) {
      webSocket.loop(); // to prevent disconnects
      delay(50);
    }
  }
}

#endif