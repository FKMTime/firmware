#include <Arduino.h>
#include <SoftwareSerial.h>
#include "StackmatTimer.h"

SoftwareSerial mySerial(1, 255, true);

StackmatTimer timer(&mySerial);
StackmatTimerState lastState;

void setup() {
    Serial.begin(19200);
    mySerial.begin(STACKMAT_TIMER_BAUD_RATE);
}

void loop() {
    timer.Update();

    if (!timer.IsConnected()) {
      Serial.println("Timer is disconnected! Make sure it is connected and turned on.");
      delay(1000);
    }
    
    if (timer.GetState() != lastState) {
        switch (timer.GetState())
        {
        case ST_Stopped:           
            Serial.printf("FINISH! Final time is %i:%02i.%03i!\n", timer.GetDisplayMinutes(), timer.GetDisplaySeconds(), timer.GetDisplayMilliseconds());
            break;
        case ST_Reset:
            Serial.println("Timer is reset!");
            break;
        case ST_Running:
            Serial.println("GO!");
            break;
        case ST_LeftHandOnTimer:
            Serial.println("Left hand on timer!");
            break;
        case ST_RightHandOnTimer:
            Serial.println("Right hand on timer!");
            break;
        case ST_BothHandsOnTimer:
            Serial.println("Both hands on timer!");
            break;
        case ST_ReadyToStart:
            Serial.println("Ready to start!");
            break;
        default:
            Serial.println("Unknown state!");
            Serial.println(timer.GetState());
            break;
        }
    }

    if (timer.GetState() == ST_Running) {
        Serial.printf("%i:%02i.%03i\n", timer.GetInterpolatedDisplayMinutes(), timer.GetInterpolatedDisplaySeconds(), timer.GetInterpolatedDisplayMilliseconds());
    }

    lastState = timer.GetState();
    delay(50);
}
