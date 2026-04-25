class Address:
    def __init__(self, city, country):
        self.city = city
        self.country = country


class Profile:
    def __init__(self, address):
        self.address = address


class User:
    def __init__(self, profile):
        self.profile = profile


class OrderProcessor:
    def ship_to_country(self, user):
        return user.profile.address.country.upper()

    def ship_to_city(self, user):
        return user.profile.address.city.lower()

    def country_then_city(self, user):
        return user.profile.address.country + " - " + user.profile.address.city
