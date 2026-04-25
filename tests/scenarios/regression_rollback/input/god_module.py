class User:
    def __init__(self, name):
        self.name = name


class Billing:
    def charge(self, user, amount):
        return f"charging {user.name} ${amount}"


class Notification:
    def send(self, user, message):
        return f"sending '{message}' to {user.name}"


def create_user(name):
    return User(name)


def charge_user(user, amount):
    return Billing().charge(user, amount)


def notify_user(user, message):
    return Notification().send(user, message)
