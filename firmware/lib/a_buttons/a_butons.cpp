#include "a_buttons.h"

template <typename T> bool compare(std::vector<T> &v1, std::vector<T> &v2) {
  std::sort(v1.begin(), v1.end());
  std::sort(v2.begin(), v2.end());
  return v1 == v2;
}

bool compareButtonsCbs(ButtonCb cb1, ButtonCb cb2) {
  return (cb1.callTime < cb2.callTime);
}

// TODO: sort buttons by their pins length
bool compareButtonsPins(Button b1, Button b2) {
  return (b1.pins.size() > b2.pins.size());
}

bool isPinsPressed(std::vector<uint8_t> pins) {
  size_t size = pins.size();
  if (size == 1)
    return digitalRead(pins.at(0)) == LOW;

  for (size_t i = 0; i < size; i++) {
    if (digitalRead(pins.at(i)) != LOW)
      return false;
  }

  return true;
}

bool isAnyPinPressed(std::vector<uint8_t> pins) {
  size_t size = pins.size();
  if (size == 1)
    return digitalRead(pins.at(0)) == LOW;

  for (size_t i = 0; i < size; i++) {
    if (digitalRead(pins.at(i)) == LOW)
      return true;
  }

  return false;
}

AButtons::AButtons() {}

size_t AButtons::loop() {
  unsigned long bPressedTime = 0;
  unsigned long lastReocCbTime = 0;

  for (size_t i = 0; i < buttons.size(); i++) {
    Button &b = buttons.at(i);
    b.disableAfterReleaseCbs = false;

    if (!isPinsPressed(b.pins))
      continue;
    if (b.afterPressCb != NULL)
      b.afterPressCb(b);
    bPressedTime = millis();

    // while holding
    while (isAnyPinPressed(b.pins)) {
      for (size_t cb = 0; cb < b.callbacks.size(); cb++) {
        ButtonCb &bcb = b.callbacks.at(cb);

        if (bcb.callback != NULL && bcb.callTime > millis() - bPressedTime)
          break;
        if (bcb.afterRelease || bcb.called)
          continue;

        if (bcb.callback == NULL) {
          if (millis() - lastReocCbTime < bcb.callTime)
            continue;

          lastReocCbTime = millis();
          bcb.reocCallback(millis() - bPressedTime);
        } else {
          bcb.callback(b);
          bcb.called = true;
        }
      }

      delay(checkDelay);
    }

    // after release
    for (size_t cb = 0; cb < b.callbacks.size(); cb++) {
      ButtonCb &bcb = b.callbacks.at(cb);
      bcb.called = false; // clear called status

      if (bcb.callTime > millis() - bPressedTime)
        break;
      if (b.disableAfterReleaseCbs)
        continue;
      if (!bcb.afterRelease)
        continue;
      if (bcb.callback == NULL)
        continue;

      bcb.callback(b);
    }

    if (b.afterReleaseCb != NULL)
      b.afterReleaseCb(b);

    lastHoldTime = millis() - bPressedTime;
    return i;
  }

  return -1;
}

Button *AButtons::getButton(size_t idx) { return &buttons.at(idx); }
int AButtons::getLastHoldTime() { return lastHoldTime; }

size_t AButtons::addButton(uint8_t _pin, callback_t _beforeReleaseCb,
                           callback_t _afterReleaseCb) {
  std::vector<uint8_t> _pins = {_pin};

  Button b = {.pins = _pins,
              .afterPressCb = _beforeReleaseCb,
              .afterReleaseCb = _afterReleaseCb};

  buttons.push_back(b);
  return buttons.size() - 1;
}

size_t AButtons::addMultiButton(std::vector<uint8_t> _pins,
                                callback_t _beforeReleaseCb,
                                callback_t _afterReleaseCb) {
  Button b = {.pins = _pins,
              .afterPressCb = _beforeReleaseCb,
              .afterReleaseCb = _afterReleaseCb};

  buttons.push_back(b);
  // sort buttons by their pins length
  std::sort(buttons.begin(), buttons.end(), compareButtonsPins);

  size_t idx = 0;
  for (size_t i = 0; i < buttons.size(); i++) {
    if (buttons.at(i).pins == _pins) {
      idx = i;
      break;
    }
  }

  return idx;
}

void AButtons::addButtonCb(size_t idx, int _callTime, bool _afterRelease,
                           callback_t callback) {
  ButtonCb cb = {.callTime = _callTime,
                 .called = false,
                 .afterRelease = _afterRelease,
                 .callback = callback};

  Button &b = buttons.at(idx);
  b.callbacks.push_back(cb);

  // sort callbacks by their calltime
  std::sort(b.callbacks.begin(), b.callbacks.end(), compareButtonsCbs);
}

void AButtons::addButtonReocCb(size_t idx, int _callInterval,
                               reoc_callback_t callback) {
  ButtonCb cb = {.callTime = _callInterval,
                 .called = false,
                 .afterRelease = false,
                 .reocCallback = callback};

  Button &b = buttons.at(idx);
  b.callbacks.push_back(cb);

  // sort callbacks by their calltime
  std::sort(b.callbacks.begin(), b.callbacks.end(), compareButtonsCbs);
}

void AButtons::testButtonClick(std::vector<uint8_t> pins, int pressTime) {
  unsigned long bPressedTime = 0;
  unsigned long lastReocCbTime = 0;

  for (size_t i = 0; i < buttons.size(); i++) {
    Button &b = buttons.at(i);
    if (!compare(b.pins, pins))
      continue;

    b.disableAfterReleaseCbs = false;
    if (b.afterPressCb != NULL)
      b.afterPressCb(b);
    bPressedTime = millis();

    // while holding
    while (millis() - bPressedTime < pressTime) {
      for (size_t cb = 0; cb < b.callbacks.size(); cb++) {
        ButtonCb &bcb = b.callbacks.at(cb);

        if (bcb.callback != NULL && bcb.callTime > millis() - bPressedTime)
          break;
        if (bcb.afterRelease || bcb.called)
          continue;

        if (bcb.callback == NULL) {
          if (millis() - lastReocCbTime < bcb.callTime)
            continue;

          lastReocCbTime = millis();
          bcb.reocCallback(millis() - bPressedTime);
        } else {
          bcb.callback(b);
          bcb.called = true;
        }
      }

      delay(checkDelay);
    }

    // after release
    for (size_t cb = 0; cb < b.callbacks.size(); cb++) {
      ButtonCb &bcb = b.callbacks.at(cb);
      bcb.called = false; // clear called status

      if (bcb.callTime > millis() - bPressedTime)
        break;
      if (b.disableAfterReleaseCbs)
        continue;
      if (!bcb.afterRelease)
        continue;
      if (bcb.callback == NULL)
        continue;

      bcb.callback(b);
    }

    if (b.afterReleaseCb != NULL)
      b.afterReleaseCb(b);
  }
}
