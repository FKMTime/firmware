#include "a_buttons.h"

bool compareButtonsCbs(ButtonCb cb1, ButtonCb cb2) 
{ 
    return (cb1.callTime < cb1.callTime); 
} 


AButtons::AButtons() {}

void AButtons::loop() {
    unsigned long bPressedTime = 0;
    unsigned long lastReocCbTime = 0;

    for(size_t i = 0; i < buttons.size(); i++) {
        Button &b = buttons.at(i);
        if (digitalRead(b.pin) != LOW) continue;
        bPressedTime = millis();

        // while holding
        while (digitalRead(b.pin) == LOW) {
            for(size_t cb = 0; cb < b.callbacks.size(); cb++) {
                ButtonCb &bcb = b.callbacks.at(cb);

                if(bcb.callback != NULL && bcb.callTime > millis() - bPressedTime) break;
                if(bcb.afterRelease || bcb.called) continue;

                if(bcb.callback == NULL) {
                    if (millis() - lastReocCbTime < bcb.callTime) continue;
                    lastReocCbTime = millis();

                    bcb.reocCallback(millis() - bPressedTime);
                } else {
                    bcb.callback();
                    bcb.called = true;
                }
            }

            delay(checkDelay);
        }

        // after unpress
        for(size_t cb = 0; cb < b.callbacks.size(); cb++) {
            ButtonCb &bcb = b.callbacks.at(cb);
            bcb.called = false; // clear called status

            if(bcb.callTime > millis() - bPressedTime) break;
            if(!bcb.afterRelease) continue;
            if(bcb.callback == NULL) continue;

            bcb.callback();
        }

        if(b.afterReleaseCb != NULL) b.afterReleaseCb();
    }
}

size_t AButtons::addButton(uint8_t _pin, callback_t _afterReleaseCb) {
    Button b = {
        .pin = _pin,
        .afterReleaseCb = _afterReleaseCb
    };

    buttons.push_back(b);
    return buttons.size() - 1;
}

void AButtons::addButtonCb(size_t idx, int _callTime, bool _afterRelease, callback_t callback) {
    ButtonCb cb = {
        .callTime = _callTime,  
        .called = false,
        .afterRelease = _afterRelease,
        .callback = callback
    };

    Button &b = buttons.at(idx);
    b.callbacks.push_back(cb);

    // sort callbacks by their calltime
    std::sort(b.callbacks.begin(), b.callbacks.end(), compareButtonsCbs);
}

void AButtons::addButtonReocCb(size_t idx, int _callInterval, reoc_callback_t callback) {
    ButtonCb cb = {
        .callTime = _callInterval,
        .called = false,
        .afterRelease = false,
        .reocCallback = callback
    };

    Button &b = buttons.at(idx);
    b.callbacks.push_back(cb);

    // sort callbacks by their calltime
    std::sort(b.callbacks.begin(), b.callbacks.end(), compareButtonsCbs);
}